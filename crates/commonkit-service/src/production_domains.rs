use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use commonkit_about_me::{ClaimCategory, ClaimInput, ProfileStore, ScopedView};
use commonkit_adapters::{
    ApmProvider, ApmProviderConfig, ArtifactStore, BwsCredentialResolver, ChezmoiProvider,
    ContentSensitivity, CredentialReference, CredentialResolver, DesiredStateProvider,
    EngramChunkAdapter, EngramChunkSetState, EngramCommandRunner, EngramGrant, EngramGrantState,
    EngramOwnerId, EngramProjectId, EngramScope, EngramTargetChunkSetDeclaration,
    EngramTargetRuntime, EngramTargetSyncMode, ExactProviderVersion, FileAdapter, FilesystemIntent,
    GitRepository, GitSyncDisposition, LocalSensitiveFileStore, LocalTargetFilesystem,
    MaterializedState, NativeProvider, NormalizedManagedPath, NormalizedResource, OpenSshConfig,
    OpenSshTransport, OwnershipRules, PlatformKeychain, PlatformKeychainCredentialResolver,
    ProcessBwsRunner, ProcessEngramCommandRunner, ProcessGitRunner,
    ProcessPlatformSecretCommandRunner, ProcessRemoteRunner, ProviderCapability, ProviderContext,
    ProviderInputs, ProviderPipeline, ProviderPlanRequest, ProviderResourcePlanner,
    RemoteProviderStager, ResourceProvenance, SecretValue, SshFileAdapter, SshTargetFilesystem,
    TargetFilesystem, build_provider_plan, materialize_mcp_client_state, validate_ownership,
};
use commonkit_config::{
    LayerSet, StyleguidePolicy, compose_layers, resolve_styleguide_selection, v1_merge_rules,
    validate_layer_content_digest,
};
use commonkit_contracts::{
    LayerDocument, LayerKind, Plan, ReceiptState, SecurityPolicy, Sha256Digest, StableId,
    StyleguideDescriptor, StyleguideSelection, assert_no_embedded_secrets, digest_domain_json,
};
use commonkit_core::{PlanDraft, RootAccess, build_plan, enforce_policy_floor};
use commonkit_reconcile::{
    Adapter, PlanStore, ReceiptError, ReceiptStore, ReconcileOutcome, Reconciler,
};
use commonkit_snapshots::{
    AuthenticatedCipher, AuthorityStore, ConsistentBackup, DatabaseId, DatabaseLifecycle,
    DurableRestore, ObjectStore, PortableAuthorityStore, PortableHeadUpdate,
    ProcessGitAuthorityPublisher, ProcessObjectCommandRunner, PromotionPlan, RestoreFailpoint,
    RestorePlan, S3CompatibleObjectStore, SnapshotError, SnapshotManifest, SnapshotService,
    SqliteBackup, StaticBackup, XChaCha20Cipher, manifest_digest,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::skill_canary::{SkillCanaryConfig, SkillCanaryRuntime};
use crate::{
    AboutMeDomain, ApplyStatus, CompositionDomain, CredentialDomain, DomainFailure,
    ExecutionResult, HeadlessDomainRegistry, PlanExecutor, RelayProviderAuthority, SnapshotDomain,
    SyncDomain,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProductionConfig {
    about_me: Option<AboutMeConfig>,
    targets: Option<TargetInventoryConfig>,
    composition: Option<CompositionConfig>,
    sync: Option<SyncConfig>,
    #[serde(default)]
    sync_targets: Vec<SyncConfig>,
    credentials: Option<CredentialConfig>,
    snapshots: Option<SnapshotConfig>,
    skill_canary: Option<SkillCanaryConfig>,
    #[serde(default)]
    engram_grants: Vec<EngramGrant>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AboutMeConfig {
    database: PathBuf,
    key_reference: CredentialReference,
    bws_executable: Option<PathBuf>,
    loadout_id: String,
    project_id: String,
    #[serde(default = "default_about_me_agent")]
    agent_id: String,
}

fn default_about_me_agent() -> String {
    "commonkit-agent".into()
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TargetInventoryConfig {
    state: PathBuf,
    #[serde(default)]
    selected: Vec<StableId>,
    entries: Vec<crate::TargetRecord>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CompositionConfig {
    layers: Vec<PathBuf>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncConfig {
    target_id: StableId,
    target_root: PathBuf,
    adapter_state: PathBuf,
    provider_artifacts: PathBuf,
    #[serde(default)]
    materialized_states: Vec<PathBuf>,
    provider_pipeline: Option<ProviderPipelineConfig>,
    styleguide: Option<StyleguideRuntimeConfig>,
    target_transport: Option<SyncTargetTransport>,
    /// Explicit facts for the managed target. Legacy local configurations may
    /// omit this and inherit the controller facts; SSH targets may not.
    target_platform: Option<SyncTargetPlatform>,
    declared_roots: Vec<NormalizedManagedPath>,
    /// Target-relative home/project root where Claude and Codex consume their
    /// relay client configuration. Required when providers declare MCP.
    relay_client_root: Option<NormalizedManagedPath>,
    protected_roots: Vec<NormalizedManagedPath>,
    case_sensitive: bool,
    target_identity_digest: Sha256Digest,
    composed_loadout_digest: Sha256Digest,
    policy_digest: Sha256Digest,
    engram: Option<EngramResourceConfig>,
    /// Runtime-derived, target-local endpoint. It is deliberately absent from
    /// portable configuration so clients cannot drift from daemon discovery.
    #[serde(skip)]
    relay_endpoint: Option<String>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EngramResourceConfig {
    project_id: EngramProjectId,
    owner_id: EngramOwnerId,
    project_root: NormalizedManagedPath,
    root: NormalizedManagedPath,
    scope: EngramScope,
    executable: Option<PathBuf>,
    auto_reconcile: Option<EngramAutoReconcileConfig>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EngramAutoReconcileConfig {
    peer_target_id: StableId,
    #[serde(default = "default_engram_interval_seconds")]
    interval_seconds: u64,
}

fn default_engram_interval_seconds() -> u64 {
    60
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StyleguideRuntimeConfig {
    selection: StyleguideSelection,
    descriptors: Vec<PathBuf>,
    manifest: PathBuf,
    lockfile: PathBuf,
    #[serde(default)]
    policy: StyleguideRuntimePolicy,
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StyleguideRuntimePolicy {
    #[serde(default)]
    denied: bool,
    pinned_skill_id: Option<StableId>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncTargetPlatform {
    operating_system: String,
    architecture: String,
}

impl SyncConfig {
    fn provider_platform(&self) -> Result<SyncTargetPlatform, DomainFailure> {
        if let Some(platform) = &self.target_platform {
            if platform.operating_system.trim().is_empty()
                || platform.architecture.trim().is_empty()
            {
                return Err(DomainFailure::InvalidRequest);
            }
            return Ok(platform.clone());
        }
        match self
            .target_transport
            .as_ref()
            .unwrap_or(&SyncTargetTransport::Local)
        {
            SyncTargetTransport::Local => Ok(SyncTargetPlatform {
                operating_system: std::env::consts::OS.to_owned(),
                architecture: std::env::consts::ARCH.to_owned(),
            }),
            SyncTargetTransport::Ssh { .. } => Err(DomainFailure::InvalidRequest),
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum SyncTargetTransport {
    Local,
    Ssh {
        #[serde(rename = "rootId")]
        root_id: StableId,
        host: String,
        user: String,
        port: u16,
        #[serde(rename = "knownHosts")]
        known_hosts: PathBuf,
        fingerprint: String,
    },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProviderPipelineConfig {
    root: PathBuf,
    source: GitProviderSource,
    providers: Vec<ConfiguredProvider>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GitProviderSource {
    repository: PathBuf,
    trusted_remote_url: String,
    revision: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "provider", rename_all = "snake_case", deny_unknown_fields)]
enum ConfiguredProvider {
    Native {
        version: ExactProviderVersion,
        files: Vec<NativeProviderFile>,
    },
    Apm {
        executable: PathBuf,
        version: ExactProviderVersion,
        manifest: PathBuf,
        lockfile: PathBuf,
        policy: PathBuf,
        targets: Vec<String>,
        #[serde(rename = "managedRoot")]
        managed_root: NormalizedManagedPath,
    },
    Chezmoi {
        executable: PathBuf,
        source: PathBuf,
        config: PathBuf,
    },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct NativeProviderFile {
    path: NormalizedManagedPath,
    source: NormalizedManagedPath,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CredentialConfig {
    root: PathBuf,
    bws_executable: Option<PathBuf>,
    destinations: Vec<CredentialDestination>,
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CredentialDestination {
    id: StableId,
    reference: CredentialReference,
    path: NormalizedManagedPath,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SnapshotConfig {
    root: PathBuf,
    /// A directory inside the portable Git kit. It contains only content-addressed descriptors;
    /// encrypted database and manifest objects remain in the configured object store.
    portable_state: PathBuf,
    key_reference: CredentialReference,
    object_store: SnapshotObjectStoreConfig,
    git_authority: Option<SnapshotGitAuthorityConfig>,
    databases: Vec<SnapshotDatabase>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SnapshotGitAuthorityConfig {
    executable: PathBuf,
    repository: PathBuf,
    trusted_remote_url: String,
    branch: String,
    staging_root: PathBuf,
    #[serde(default)]
    bootstrap: bool,
}
#[derive(Clone, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum SnapshotObjectStoreConfig {
    Local,
    S3 {
        executable: PathBuf,
        endpoint: String,
        bucket: String,
        prefix: String,
    },
}
#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SnapshotDatabase {
    id: DatabaseId,
    path: PathBuf,
    target_id: String,
    /// Explicit local observations of this database on other configured targets. Promotion is
    /// unavailable when either side cannot be inspected; request-provided digests are never an
    /// authority substitute.
    #[serde(default)]
    observed_paths: BTreeMap<String, PathBuf>,
    format: SnapshotSourceFormat,
    lifecycle: DatabaseLifecycleConfig,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct DatabaseLifecycleConfig {
    stop: LifecycleCommand,
    start: LifecycleCommand,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct LifecycleCommand {
    executable: PathBuf,
    #[serde(default)]
    args: Vec<String>,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SnapshotSourceFormat {
    Sqlite,
    File,
}

pub struct ProductionDomainRegistry {
    pub about_me: Option<Arc<dyn AboutMeDomain>>,
    pub composition: Option<Arc<dyn CompositionDomain>>,
    pub sync: Option<Arc<dyn SyncDomain>>,
    pub credentials: Option<Arc<dyn CredentialDomain>>,
    pub snapshots: Option<Arc<dyn SnapshotDomain>>,
    pub(crate) skill_canary: Option<Arc<SkillCanaryRuntime>>,
    pub targets: Option<Arc<crate::TargetInventory>>,
    pub target_sync_domains: BTreeMap<StableId, Arc<dyn SyncDomain>>,
    sync_configs: BTreeMap<StableId, SyncConfig>,
    ssh_execution: Option<(PathBuf, ProductionSshTarget)>,
    pub(crate) engram_periodic_job: Option<ProductionEngramPeriodicJob>,
}

#[derive(Clone)]
pub(crate) struct ProductionEngramPeriodicJob {
    domain: Arc<dyn SyncDomain>,
    peer_target_id: StableId,
    pub(crate) interval_seconds: u64,
}

impl ProductionEngramPeriodicJob {
    pub(crate) fn reconcile_once(&self) -> Result<Value, DomainFailure> {
        self.domain.engram_reconcile(serde_json::json!({
            "confirmed": true,
            "confirmationId": format!("engram-periodic-{:032x}", rand::random::<u128>()),
            "peerTargetId": self.peer_target_id
        }))
    }
}

pub trait ProductionSshTransportFactory: Send + Sync + 'static {
    fn open(
        &self,
        config: &ProductionSshTarget,
    ) -> Result<Box<dyn commonkit_adapters::SshFilesystemTransport + Send>, DomainFailure>;
}

#[derive(Clone)]
pub struct ProductionSshTarget {
    root_id: StableId,
    host: String,
    user: String,
    port: u16,
    known_hosts: PathBuf,
    fingerprint: String,
}

struct ProcessSshTransportFactory;
impl ProductionSshTransportFactory for ProcessSshTransportFactory {
    fn open(
        &self,
        config: &ProductionSshTarget,
    ) -> Result<Box<dyn commonkit_adapters::SshFilesystemTransport + Send>, DomainFailure> {
        let transport = OpenSshTransport::new(
            OpenSshConfig::new(
                &config.host,
                &config.user,
                config.port,
                config.known_hosts.clone(),
                &config.fingerprint,
            )
            .map_err(|_| DomainFailure::OperationFailed)?,
            ProcessRemoteRunner,
        )
        .map_err(|_| DomainFailure::OperationFailed)?;
        Ok(Box::new(transport))
    }
}

pub struct ProductionSshPlanExecutor {
    plan_store: Arc<PlanStore>,
    receipts: ReceiptStore,
    adapter_state: PathBuf,
    target: ProductionSshTarget,
    factory: Arc<dyn ProductionSshTransportFactory>,
    lock: Mutex<()>,
}

impl ProductionSshPlanExecutor {
    pub fn with_factory(
        plan_store: Arc<PlanStore>,
        receipt_root: impl AsRef<Path>,
        adapter_state: PathBuf,
        target: ProductionSshTarget,
        factory: Arc<dyn ProductionSshTransportFactory>,
    ) -> Result<Self, ProductionDomainError> {
        Ok(Self {
            plan_store,
            receipts: ReceiptStore::open(receipt_root)
                .map_err(|_| ProductionDomainError::UnsafeConfig)?,
            adapter_state,
            target,
            factory,
            lock: Mutex::new(()),
        })
    }

    fn adapter(&self) -> Result<Vec<Box<dyn Adapter>>, DomainFailure> {
        let transport = self.factory.open(&self.target)?;
        Ok(vec![Box::new(
            SshFileAdapter::open(self.target.root_id.clone(), &self.adapter_state, transport)
                .map_err(|_| DomainFailure::OperationFailed)?,
        )])
    }

    fn execute_inner(
        &self,
        plan: &Plan,
        confirmation: &StableId,
    ) -> Result<ReconcileOutcome, DomainFailure> {
        let durable = self
            .plan_store
            .load(&plan.id)
            .map_err(|_| DomainFailure::OperationFailed)?;
        if durable != *plan {
            return Err(DomainFailure::StalePlan);
        }
        let run_digest =
            digest_domain_json("commonkit.production-ssh-run.v1", &(&plan.id, confirmation))
                .map_err(|_| DomainFailure::OperationFailed)?;
        let run_id = StableId::parse(format!("run-{}", &run_digest.as_str()[7..55]))
            .map_err(|_| DomainFailure::OperationFailed)?;
        let mut adapters = self.adapter()?;
        match self.receipts.load(run_id.clone()) {
            Ok(receipt) if matches!(receipt.receipt().state, ReceiptState::Succeeded) => {
                Ok(ReconcileOutcome::Succeeded)
            }
            Ok(receipt)
                if matches!(
                    receipt.receipt().state,
                    ReceiptState::RolledBack | ReceiptState::Canceled
                ) =>
            {
                Ok(ReconcileOutcome::RolledBack)
            }
            Ok(_) => Reconciler::with_store(&self.receipts)
                .recover_run(run_id, &durable, &mut adapters)
                .map_err(|_| DomainFailure::OperationFailed),
            Err(ReceiptError::NotFound(_)) => Reconciler::with_store(&self.receipts)
                .execute(&durable, run_id, &mut adapters)
                .map_err(|_| DomainFailure::OperationFailed),
            Err(ReceiptError::Io(ref error)) if error.kind() == std::io::ErrorKind::NotFound => {
                Reconciler::with_store(&self.receipts)
                    .execute(&durable, run_id, &mut adapters)
                    .map_err(|_| DomainFailure::OperationFailed)
            }
            Err(_) => Err(DomainFailure::OperationFailed),
        }
    }
}

impl PlanExecutor for ProductionSshPlanExecutor {
    fn execute(&self, plan: &Plan, confirmation_id: &StableId) -> ExecutionResult {
        let result = self
            .lock
            .lock()
            .map_err(|_| DomainFailure::OperationFailed)
            .and_then(|_| self.execute_inner(plan, confirmation_id));
        match result {
            Ok(ReconcileOutcome::Succeeded) => ExecutionResult {
                status: ApplyStatus::Succeeded,
                failure_code: None,
            },
            Ok(ReconcileOutcome::RolledBack | ReconcileOutcome::Canceled) => ExecutionResult {
                status: ApplyStatus::RolledBack,
                failure_code: None,
            },
            Ok(ReconcileOutcome::RollbackFailed) | Err(_) => ExecutionResult {
                status: ApplyStatus::Failed,
                failure_code: Some(StableId::parse("remote_execution_failed").expect("static ID")),
            },
        }
    }
}

impl ProductionDomainRegistry {
    pub fn engram_periodic_interval_seconds(&self) -> Option<u64> {
        self.engram_periodic_job
            .as_ref()
            .map(|job| job.interval_seconds)
    }

    pub fn reconcile_engram_periodic_once(&self) -> Result<Option<Value>, DomainFailure> {
        self.engram_periodic_job
            .as_ref()
            .map(ProductionEngramPeriodicJob::reconcile_once)
            .transpose()
    }

    pub fn target_executors(
        &self,
        plan_store: Arc<PlanStore>,
        receipt_root: impl AsRef<Path>,
    ) -> Result<BTreeMap<StableId, Arc<dyn PlanExecutor>>, ProductionDomainError> {
        self.target_executors_with_factory(
            plan_store,
            receipt_root,
            Arc::new(ProcessSshTransportFactory),
        )
    }

    pub fn target_executors_with_factory(
        &self,
        plan_store: Arc<PlanStore>,
        receipt_root: impl AsRef<Path>,
        factory: Arc<dyn ProductionSshTransportFactory>,
    ) -> Result<BTreeMap<StableId, Arc<dyn PlanExecutor>>, ProductionDomainError> {
        let mut executors = BTreeMap::new();
        for (id, config) in &self.sync_configs {
            let executor: Arc<dyn PlanExecutor> = match config
                .target_transport
                .as_ref()
                .unwrap_or(&SyncTargetTransport::Local)
            {
                SyncTargetTransport::Local => Arc::new(
                    crate::LocalPlanExecutor::open(
                        plan_store.clone(),
                        receipt_root.as_ref(),
                        &config.target_root,
                        &config.adapter_state,
                    )
                    .map_err(|_| ProductionDomainError::UnsafeConfig)?,
                ),
                transport @ SyncTargetTransport::Ssh { .. } => Arc::new(
                    ProductionSshPlanExecutor::with_factory(
                        plan_store.clone(),
                        receipt_root.as_ref(),
                        config.adapter_state.clone(),
                        production_ssh_target(transport)
                            .map_err(|_| ProductionDomainError::UnsafeConfig)?,
                        factory.clone(),
                    )
                    .map_err(|_| ProductionDomainError::UnsafeConfig)?,
                ),
            };
            executors.insert(id.clone(), executor);
        }
        Ok(executors)
    }

    pub fn ssh_executor(
        &self,
        plan_store: Arc<PlanStore>,
        receipt_root: impl AsRef<Path>,
    ) -> Result<Option<Arc<dyn PlanExecutor>>, ProductionDomainError> {
        self.ssh_executor_with_factory(
            plan_store,
            receipt_root,
            Arc::new(ProcessSshTransportFactory),
        )
    }

    pub fn ssh_executor_with_factory(
        &self,
        plan_store: Arc<PlanStore>,
        receipt_root: impl AsRef<Path>,
        factory: Arc<dyn ProductionSshTransportFactory>,
    ) -> Result<Option<Arc<dyn PlanExecutor>>, ProductionDomainError> {
        self.ssh_execution
            .as_ref()
            .map(|(state, target)| {
                ProductionSshPlanExecutor::with_factory(
                    plan_store,
                    receipt_root,
                    state.clone(),
                    target.clone(),
                    factory,
                )
                .map(|executor| Arc::new(executor) as Arc<dyn PlanExecutor>)
            })
            .transpose()
    }

    pub fn load_optional(
        config_path: &Path,
        plan_store: Arc<PlanStore>,
        receipt_root: PathBuf,
    ) -> Result<Self, ProductionDomainError> {
        if !config_path.exists() {
            return Ok(Self {
                about_me: None,
                composition: None,
                sync: None,
                credentials: None,
                snapshots: None,
                skill_canary: None,
                targets: None,
                target_sync_domains: BTreeMap::new(),
                sync_configs: BTreeMap::new(),
                ssh_execution: None,
                engram_periodic_job: None,
            });
        }
        Self::load(config_path, plan_store, receipt_root)
    }
    pub fn load_optional_with_relay_endpoint(
        config_path: &Path,
        plan_store: Arc<PlanStore>,
        receipt_root: PathBuf,
        relay_address: std::net::SocketAddr,
    ) -> Result<Self, ProductionDomainError> {
        if !relay_address.ip().is_loopback() {
            return Err(ProductionDomainError::UnsafeConfig);
        }
        if !config_path.exists() {
            return Self::load_optional(config_path, plan_store, receipt_root);
        }
        Self::load_with_ssh_factory_and_relay_endpoint(
            config_path,
            plan_store,
            receipt_root,
            Arc::new(ProcessSshTransportFactory),
            Some(format!("http://127.0.0.1:{}/mcp", relay_address.port())),
        )
    }
    pub fn load(
        config_path: &Path,
        plan_store: Arc<PlanStore>,
        receipt_root: PathBuf,
    ) -> Result<Self, ProductionDomainError> {
        Self::load_with_ssh_factory(
            config_path,
            plan_store,
            receipt_root,
            Arc::new(ProcessSshTransportFactory),
        )
    }

    pub fn load_with_ssh_factory(
        config_path: &Path,
        plan_store: Arc<PlanStore>,
        receipt_root: PathBuf,
        ssh_factory: Arc<dyn ProductionSshTransportFactory>,
    ) -> Result<Self, ProductionDomainError> {
        Self::load_with_ssh_factory_and_relay_endpoint(
            config_path,
            plan_store,
            receipt_root,
            ssh_factory,
            None,
        )
    }

    fn load_with_ssh_factory_and_relay_endpoint(
        config_path: &Path,
        plan_store: Arc<PlanStore>,
        receipt_root: PathBuf,
        ssh_factory: Arc<dyn ProductionSshTransportFactory>,
        relay_endpoint: Option<String>,
    ) -> Result<Self, ProductionDomainError> {
        let metadata = fs::symlink_metadata(config_path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(ProductionDomainError::UnsafeConfig);
        }
        let mut config: ProductionConfig = serde_json::from_slice(&fs::read(config_path)?)?;
        validate_engram_grants(&config.engram_grants)?;
        if let Some(endpoint) = relay_endpoint {
            if let Some(sync) = config.sync.as_mut() {
                sync.relay_endpoint = Some(endpoint.clone());
            }
            for sync in &mut config.sync_targets {
                sync.relay_endpoint = Some(endpoint.clone());
            }
        }
        let mut sync_configs = BTreeMap::new();
        for sync in config
            .sync
            .iter()
            .chain(config.sync_targets.iter())
            .cloned()
        {
            let id = sync.target_id.clone();
            if sync_configs.insert(id, sync).is_some() {
                return Err(ProductionDomainError::UnsafeConfig);
            }
        }
        let targets = if let Some(targets) = config.targets.as_ref() {
            if !targets.state.is_absolute() {
                return Err(ProductionDomainError::UnsafeConfig);
            }
            Some(
                crate::TargetInventory::open(
                    &targets.state,
                    targets.entries.clone(),
                    Some(targets.selected.clone()),
                )
                .map(Arc::new)
                .map_err(|_| ProductionDomainError::UnsafeConfig)?,
            )
        } else {
            if sync_configs.is_empty() {
                None
            } else {
                let entries = sync_configs
                    .values()
                    .map(|sync| {
                        let transport = match sync.target_transport.as_ref() {
                            Some(SyncTargetTransport::Ssh {
                                host, user, port, ..
                            }) => crate::TargetTransport::Ssh {
                                host: host.clone(),
                                user: user.clone(),
                                port: *port,
                            },
                            _ => crate::TargetTransport::Local,
                        };
                        crate::TargetRecord {
                            id: sync.target_id.clone(),
                            transport,
                            identity_digest: sync.target_identity_digest.clone(),
                        }
                    })
                    .collect();
                Some(
                    crate::TargetInventory::open(
                        config_path.with_extension("targets.json"),
                        entries,
                        None,
                    )
                    .map(Arc::new)
                    .map_err(|_| ProductionDomainError::UnsafeConfig)?,
                )
            }
        };
        let ssh_execution =
            config
                .sync
                .as_ref()
                .and_then(|sync| match sync.target_transport.as_ref() {
                    Some(SyncTargetTransport::Ssh {
                        root_id,
                        host,
                        user,
                        port,
                        known_hosts,
                        fingerprint,
                    }) => Some((
                        sync.adapter_state.clone(),
                        ProductionSshTarget {
                            root_id: root_id.clone(),
                            host: host.clone(),
                            user: user.clone(),
                            port: *port,
                            known_hosts: known_hosts.clone(),
                            fingerprint: fingerprint.clone(),
                        },
                    )),
                    _ => None,
                });
        let composition = config
            .composition
            .map(
                |config| -> Result<Arc<dyn CompositionDomain>, ProductionDomainError> {
                    if config.layers.is_empty()
                        || config.layers.iter().any(|path| !path.is_absolute())
                    {
                        return Err(ProductionDomainError::EmptyCapability);
                    }
                    let domain = ProductionCompositionDomain { config };
                    domain
                        .compose()
                        .map_err(|_| ProductionDomainError::InvalidComposition)?;
                    Ok(Arc::new(domain))
                },
            )
            .transpose()?;
        let about_me = config
            .about_me
            .map(
                |config| -> Result<Arc<dyn AboutMeDomain>, ProductionDomainError> {
                    if !config.database.is_absolute()
                        || config.loadout_id.trim().is_empty()
                        || config.project_id.trim().is_empty()
                    {
                        return Err(ProductionDomainError::UnsafeConfig);
                    }
                    let domain = ProductionAboutMeDomain { config };
                    domain
                        .store()
                        .map_err(|_| ProductionDomainError::UnsafeConfig)?;
                    Ok(Arc::new(domain))
                },
            )
            .transpose()?;
        let mut target_sync_domains = BTreeMap::new();
        let shared_sync_configs = Arc::new(sync_configs.clone());
        let shared_engram_grants = Arc::new(config.engram_grants.clone());
        let engram_lock = Arc::new(Mutex::new(()));
        for (id, sync_config) in &sync_configs {
            validate_sync_config(sync_config)?;
            target_sync_domains.insert(
                id.clone(),
                Arc::new(ProductionSyncDomain {
                    config: sync_config.clone(),
                    plan_store: plan_store.clone(),
                    receipt_root: receipt_root.clone(),
                    ssh_factory: ssh_factory.clone(),
                    sync_configs: shared_sync_configs.clone(),
                    engram_grants: shared_engram_grants.clone(),
                    engram_lock: engram_lock.clone(),
                }) as Arc<dyn SyncDomain>,
            );
        }
        let periodic = sync_configs
            .iter()
            .filter_map(|(target_id, sync)| {
                sync.engram
                    .as_ref()
                    .and_then(|engram| engram.auto_reconcile.as_ref())
                    .map(|schedule| (target_id, schedule))
            })
            .collect::<Vec<_>>();
        if periodic.len() > 1 {
            return Err(ProductionDomainError::UnsafeConfig);
        }
        let engram_periodic_job = periodic
            .first()
            .map(|(target_id, schedule)| {
                if schedule.interval_seconds < 60
                    || schedule.peer_target_id.as_str() == target_id.as_str()
                    || !sync_configs.contains_key(&schedule.peer_target_id)
                {
                    return Err(ProductionDomainError::UnsafeConfig);
                }
                Ok(ProductionEngramPeriodicJob {
                    domain: target_sync_domains
                        .get(*target_id)
                        .cloned()
                        .ok_or(ProductionDomainError::UnsafeConfig)?,
                    peer_target_id: schedule.peer_target_id.clone(),
                    interval_seconds: schedule.interval_seconds,
                })
            })
            .transpose()?;
        let sync = config
            .sync
            .as_ref()
            .and_then(|legacy| target_sync_domains.get(&legacy.target_id).cloned());
        let credentials = config
            .credentials
            .map(
                |config| -> Result<Arc<dyn CredentialDomain>, ProductionDomainError> {
                    if config.destinations.is_empty() || !config.root.is_absolute() {
                        return Err(ProductionDomainError::EmptyCapability);
                    }
                    if let Some(executable) = &config.bws_executable {
                        let metadata = fs::symlink_metadata(executable)
                            .map_err(|_| ProductionDomainError::UnsafeConfig)?;
                        if !executable.is_absolute()
                            || !metadata.is_file()
                            || metadata.file_type().is_symlink()
                        {
                            return Err(ProductionDomainError::UnsafeConfig);
                        }
                    }
                    fs::create_dir_all(&config.root)?;
                    let domain = Arc::new(ProductionCredentialDomain {
                        root: config.root,
                        bws_executable: config.bws_executable,
                        destinations: unique_destinations(config.destinations)?,
                        lock: Mutex::new(()),
                        directory_sync: Arc::new(sync_directory_io),
                    });
                    domain
                        .recover_unfinished()
                        .map_err(|_| ProductionDomainError::UnsafeConfig)?;
                    Ok(domain)
                },
            )
            .transpose()?;
        let snapshots = config
            .snapshots
            .map(
                |config| -> Result<Arc<dyn SnapshotDomain>, ProductionDomainError> {
                    if config.databases.is_empty()
                        || !config.root.is_absolute()
                        || !config.portable_state.is_absolute()
                    {
                        return Err(ProductionDomainError::EmptyCapability);
                    }
                    validate_object_store(&config.object_store)?;
                    let defer_authority_bootstrap = sync_configs.values().any(|sync| {
                        sync.provider_pipeline.as_ref().is_some_and(|pipeline| {
                            config
                                .portable_state
                                .starts_with(&pipeline.source.repository)
                        })
                    });
                    fs::create_dir_all(config.root.join("objects"))?;
                    fs::create_dir_all(config.root.join("manifests"))?;
                    fs::create_dir_all(config.portable_state.join("snapshots"))?;
                    let databases = unique_databases(config.databases)?;
                    for database in databases.values() {
                        if !database.path.is_absolute()
                            || database.target_id.is_empty()
                            || database.observed_paths.iter().any(|(target, path)| {
                                target.is_empty()
                                    || target == &database.target_id
                                    || !path.is_absolute()
                            })
                        {
                            return Err(ProductionDomainError::UnsafeConfig);
                        }
                        validate_lifecycle(&database.lifecycle)?;
                    }
                    let domain = ProductionSnapshotDomain {
                        root: config.root,
                        portable_state: config.portable_state,
                        key_reference: config.key_reference,
                        object_store: config.object_store,
                        git_authority: config.git_authority,
                        databases,
                        lock: Mutex::new(()),
                    };
                    if domain.git_authority.is_some() {
                        let mut publisher = domain
                            .git_authority_publisher()
                            .map_err(|_| ProductionDomainError::UnsafeConfig)?;
                        // A remote branch and its immutable authority anchor may have advanced
                        // atomically before this process was interrupted. Recover that authenticated
                        // publication intent before comparing the stale checkout to the remote.
                        publisher
                            .recover_pending_publication()
                            .map_err(|_| ProductionDomainError::UnsafeConfig)?;
                        let checked_out = publisher
                            .checked_out_revision()
                            .map_err(|_| ProductionDomainError::UnsafeConfig)?;
                        let trusted_remote = publisher
                            .trusted_remote_revision()
                            .map_err(|_| ProductionDomainError::UnsafeConfig)?;
                        if checked_out != trusted_remote {
                            return Err(ProductionDomainError::UnsafeConfig);
                        }
                    }
                    let cipher = domain
                        .cipher()
                        .map_err(|_| ProductionDomainError::UnsafeConfig)?;
                    for database in domain.databases.values() {
                        let authority_exists = domain
                            .portable_state
                            .join("authority")
                            .join(format!("{}.json", database.id))
                            .exists();
                        if defer_authority_bootstrap && !authority_exists {
                            continue;
                        }
                        domain
                            .initialize_authority(database, &cipher)
                            .map_err(|_| ProductionDomainError::UnsafeConfig)?;
                    }
                    domain.recover_unfinished()?;
                    Ok(Arc::new(domain))
                },
            )
            .transpose()?;
        let skill_canary = config
            .skill_canary
            .map(|config| {
                SkillCanaryRuntime::open(config, &receipt_root.join("skill-canary"))
                    .map(Arc::new)
                    .map_err(|_| ProductionDomainError::UnsafeConfig)
            })
            .transpose()?;
        Ok(Self {
            about_me,
            composition,
            sync,
            credentials,
            snapshots,
            skill_canary,
            targets,
            target_sync_domains,
            sync_configs,
            ssh_execution,
            engram_periodic_job,
        })
    }
    pub fn into_headless(self) -> HeadlessDomainRegistry {
        HeadlessDomainRegistry {
            about_me: self.about_me,
            composition: self.composition,
            sync: self.sync,
            credentials: self.credentials,
            snapshots: self.snapshots,
        }
    }
}

fn validate_sync_config(config: &SyncConfig) -> Result<(), ProductionDomainError> {
    let configured_pipeline = config.provider_pipeline.as_ref().is_some_and(|pipeline| {
        pipeline.root.is_absolute()
            && pipeline.source.repository.is_absolute()
            && !pipeline.providers.is_empty()
    });
    if (config.materialized_states.is_empty() && !configured_pipeline)
        || (!config.materialized_states.is_empty() && configured_pipeline)
        || config.declared_roots.is_empty()
        || !config.target_root.is_absolute()
        || !config.adapter_state.is_absolute()
        || !config.provider_artifacts.is_absolute()
    {
        return Err(ProductionDomainError::EmptyCapability);
    }
    config
        .provider_platform()
        .map_err(|_| ProductionDomainError::UnsafeConfig)?;
    if let Some(engram) = &config.engram {
        let project_root = engram.project_root.as_str();
        let chunk_root = engram.root.as_str();
        if chunk_root != project_root
            && !chunk_root
                .strip_prefix(project_root)
                .is_some_and(|suffix| suffix.starts_with('/'))
        {
            return Err(ProductionDomainError::UnsafeConfig);
        }
        match config
            .target_transport
            .as_ref()
            .unwrap_or(&SyncTargetTransport::Local)
        {
            SyncTargetTransport::Local => {
                let executable = engram
                    .executable
                    .as_ref()
                    .ok_or(ProductionDomainError::UnsafeConfig)?;
                let metadata = fs::symlink_metadata(executable)
                    .map_err(|_| ProductionDomainError::UnsafeConfig)?;
                if !executable.is_absolute()
                    || metadata.file_type().is_symlink()
                    || !metadata.is_file()
                {
                    return Err(ProductionDomainError::UnsafeConfig);
                }
            }
            SyncTargetTransport::Ssh { .. } if engram.executable.is_some() => {
                return Err(ProductionDomainError::UnsafeConfig);
            }
            SyncTargetTransport::Ssh { .. } => {}
        }
    }
    Ok(())
}

fn validate_engram_grants(grants: &[EngramGrant]) -> Result<(), ProductionDomainError> {
    let mut ids = BTreeSet::new();
    let mut relationships = BTreeSet::new();
    for grant in grants {
        if grant.grantor == grant.grantee || !ids.insert(grant.id.clone()) {
            return Err(ProductionDomainError::UnsafeConfig);
        }
        let mut principals = [grant.grantor.as_str(), grant.grantee.as_str()];
        principals.sort_unstable();
        if !relationships.insert((
            grant.project_id.as_str().to_owned(),
            principals[0].to_owned(),
            principals[1].to_owned(),
        )) {
            return Err(ProductionDomainError::UnsafeConfig);
        }
    }
    Ok(())
}

fn unique_destinations(
    values: Vec<CredentialDestination>,
) -> Result<BTreeMap<StableId, CredentialDestination>, ProductionDomainError> {
    let mut output = BTreeMap::new();
    let mut paths = BTreeSet::new();
    for value in values {
        if value.path.as_str() == CREDENTIAL_STATE_DIRECTORY
            || value
                .path
                .as_str()
                .starts_with(&format!("{CREDENTIAL_STATE_DIRECTORY}/"))
        {
            return Err(ProductionDomainError::UnsafeConfig);
        }
        let path = value.path.as_str();
        if paths.iter().any(|existing: &String| {
            existing == path
                || existing
                    .strip_prefix(path)
                    .is_some_and(|suffix| suffix.starts_with('/'))
                || path
                    .strip_prefix(existing)
                    .is_some_and(|suffix| suffix.starts_with('/'))
        }) {
            return Err(ProductionDomainError::UnsafeConfig);
        }
        paths.insert(path.to_owned());
        if output.insert(value.id.clone(), value).is_some() {
            return Err(ProductionDomainError::DuplicateId);
        }
    }
    Ok(output)
}
fn unique_databases(
    values: Vec<SnapshotDatabase>,
) -> Result<BTreeMap<String, SnapshotDatabase>, ProductionDomainError> {
    let mut output = BTreeMap::new();
    for value in values {
        if output.insert(value.id.to_string(), value).is_some() {
            return Err(ProductionDomainError::DuplicateId);
        }
    }
    Ok(output)
}
fn validate_object_store(config: &SnapshotObjectStoreConfig) -> Result<(), ProductionDomainError> {
    match config {
        SnapshotObjectStoreConfig::Local => Ok(()),
        SnapshotObjectStoreConfig::S3 {
            executable,
            endpoint,
            bucket,
            prefix,
        } => {
            if !executable.is_absolute() {
                return Err(ProductionDomainError::UnsafeConfig);
            }
            S3CompatibleObjectStore::new(
                ProcessObjectCommandRunner::new(executable),
                endpoint.clone(),
                bucket.clone(),
                prefix.clone(),
            )
            .map(|_| ())
            .map_err(|_| ProductionDomainError::UnsafeConfig)
        }
    }
}

fn validate_lifecycle(config: &DatabaseLifecycleConfig) -> Result<(), ProductionDomainError> {
    for command in [&config.stop, &config.start] {
        if !command.executable.is_absolute() {
            return Err(ProductionDomainError::UnsafeConfig);
        }
        let metadata = fs::symlink_metadata(&command.executable)
            .map_err(|_| ProductionDomainError::UnsafeConfig)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(ProductionDomainError::UnsafeConfig);
        }
    }
    Ok(())
}

struct ProductionCompositionDomain {
    config: CompositionConfig,
}

impl ProductionCompositionDomain {
    fn result(&self) -> Result<commonkit_config::CompositionResult, DomainFailure> {
        let mut layers = Vec::with_capacity(self.config.layers.len());
        for path in &self.config.layers {
            let metadata =
                fs::symlink_metadata(path).map_err(|_| DomainFailure::OperationFailed)?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(DomainFailure::OperationFailed);
            }
            let value: Value = serde_json::from_slice(
                &fs::read(path).map_err(|_| DomainFailure::OperationFailed)?,
            )
            .map_err(|_| DomainFailure::OperationFailed)?;
            assert_no_embedded_secrets(&value).map_err(|_| DomainFailure::OperationFailed)?;
            let layer = serde_json::from_value::<LayerDocument>(value)
                .map_err(|_| DomainFailure::OperationFailed)?;
            validate_layer_content_digest(&layer).map_err(|_| DomainFailure::OperationFailed)?;
            layers.push(layer);
        }
        let layers = LayerSet::new(layers).map_err(|_| DomainFailure::OperationFailed)?;
        let result = compose_layers(&layers, &v1_merge_rules())
            .map_err(|_| DomainFailure::OperationFailed)?;
        let organization = layers
            .iter()
            .find(|layer| layer.kind == LayerKind::OrganizationPolicy)
            .and_then(|layer| layer.spec.get("securityPolicy"))
            .map(|value| serde_json::from_value::<SecurityPolicy>(value.clone()))
            .transpose()
            .map_err(|_| DomainFailure::OperationFailed)?
            .unwrap_or_default();
        let effective = result
            .spec
            .get("securityPolicy")
            .map(|value| serde_json::from_value::<SecurityPolicy>(value.clone()))
            .transpose()
            .map_err(|_| DomainFailure::OperationFailed)?
            .unwrap_or_default();
        enforce_policy_floor(&organization, &effective)
            .map_err(|_| DomainFailure::OperationFailed)?;
        Ok(result)
    }
}

impl CompositionDomain for ProductionCompositionDomain {
    fn compose(&self) -> Result<Value, DomainFailure> {
        let result = self.result()?;
        serde_json::to_value(serde_json::json!({
            "spec": result.spec,
            "specDigest": result.spec_digest,
            "trace": result.trace,
            "lock": result.lock,
        }))
        .map_err(|_| DomainFailure::OperationFailed)
    }

    fn explain(&self, pointer: &str) -> Result<Value, DomainFailure> {
        self.result()?
            .trace
            .entries
            .get(pointer)
            .cloned()
            .map(|entry| serde_json::json!(entry))
            .ok_or(DomainFailure::InvalidRequest)
    }

    fn policy_summary(&self) -> Result<Value, DomainFailure> {
        self.result()?;
        Ok(serde_json::json!({"state":"valid", "violations":[]}))
    }
}

struct ProductionSyncDomain {
    config: SyncConfig,
    plan_store: Arc<PlanStore>,
    receipt_root: PathBuf,
    ssh_factory: Arc<dyn ProductionSshTransportFactory>,
    sync_configs: Arc<BTreeMap<StableId, SyncConfig>>,
    engram_grants: Arc<Vec<EngramGrant>>,
    engram_lock: Arc<Mutex<()>>,
}

enum ProductionEngramTarget {
    Local {
        filesystem: LocalTargetFilesystem,
        executable: PathBuf,
        project: PathBuf,
        project_id: EngramProjectId,
    },
    Ssh {
        filesystem: SshTargetFilesystem<Box<dyn commonkit_adapters::SshFilesystemTransport + Send>>,
        project_id: EngramProjectId,
        project_root: NormalizedManagedPath,
    },
}

impl ProductionEngramTarget {
    fn filesystem(&self) -> &dyn TargetFilesystem {
        match self {
            Self::Local { filesystem, .. } => filesystem,
            Self::Ssh { filesystem, .. } => filesystem,
        }
    }

    fn sync(&self, mode: EngramTargetSyncMode) -> Result<(), DomainFailure> {
        match self {
            Self::Local {
                executable,
                project,
                project_id,
                ..
            } => {
                let runner = ProcessEngramCommandRunner::from_path(executable);
                let arguments = match mode {
                    EngramTargetSyncMode::Export => {
                        vec!["sync", "--project", project_id.as_str()]
                    }
                    EngramTargetSyncMode::Import => {
                        vec!["sync", "--import", "--project", project_id.as_str()]
                    }
                };
                let output = runner
                    .run(project, &arguments)
                    .map_err(|_| DomainFailure::OperationFailed)?;
                if output.success {
                    Ok(())
                } else {
                    Err(DomainFailure::OperationFailed)
                }
            }
            Self::Ssh {
                filesystem,
                project_id,
                project_root,
            } => filesystem
                .sync_engram(project_id, project_root, mode)
                .map_err(|_| DomainFailure::OperationFailed),
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct EngramReconcileRequest {
    confirmed: bool,
    confirmation_id: StableId,
    peer_target_id: StableId,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProductionEngramReceipt {
    run_id: StableId,
    confirmation_id: StableId,
    reconciliation: commonkit_adapters::EngramTargetReconciliationReceipt,
    imports_completed: bool,
}

#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProviderAuthorityRecord {
    plan: Plan,
    states: Vec<MaterializedState>,
    relay_endpoint: Option<String>,
    executable_digests: BTreeMap<PathBuf, Sha256Digest>,
    external_state_digests: BTreeMap<PathBuf, Sha256Digest>,
    provider_input_digests: BTreeMap<PathBuf, Sha256Digest>,
    provider_configuration_digest: Sha256Digest,
    source_revision: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AuthenticatedProviderAuthority {
    schema: String,
    algorithm: String,
    ciphertext: Vec<u8>,
}

impl ProductionSyncDomain {
    fn configured_engram_status(&self) -> Result<Option<Value>, DomainFailure> {
        let Some(config) = &self.config.engram else {
            return Ok(None);
        };
        let declaration = EngramTargetChunkSetDeclaration {
            target_id: self.config.target_id.clone(),
            project_id: config.project_id.clone(),
            owner_id: config.owner_id.clone(),
            project_root: config.project_root.clone(),
            root: config.root.clone(),
            scope: config.scope,
        };
        let status = match self
            .config
            .target_transport
            .as_ref()
            .unwrap_or(&SyncTargetTransport::Local)
        {
            SyncTargetTransport::Local => {
                let filesystem =
                    LocalTargetFilesystem::open(&self.config.target_root, RootAccess::ReadOnly)
                        .map_err(|_| DomainFailure::OperationFailed)?;
                EngramChunkAdapter::status_target(&filesystem, &declaration)
            }
            transport @ SyncTargetTransport::Ssh { root_id, .. } => {
                let filesystem = SshTargetFilesystem::new(
                    root_id.clone(),
                    self.ssh_factory.open(&production_ssh_target(transport)?)?,
                );
                EngramChunkAdapter::status_target(&filesystem, &declaration)
            }
        }
        .map_err(|_| DomainFailure::VerificationFailed)?;
        if status.state != EngramChunkSetState::InSync {
            return Err(DomainFailure::VerificationFailed);
        }
        serde_json::to_value(status)
            .map(Some)
            .map_err(|_| DomainFailure::OperationFailed)
    }

    fn engram_declaration(
        config: &SyncConfig,
    ) -> Result<EngramTargetChunkSetDeclaration, DomainFailure> {
        let engram = config
            .engram
            .as_ref()
            .ok_or(DomainFailure::OperationFailed)?;
        Ok(EngramTargetChunkSetDeclaration {
            target_id: config.target_id.clone(),
            project_id: engram.project_id.clone(),
            owner_id: engram.owner_id.clone(),
            project_root: engram.project_root.clone(),
            root: engram.root.clone(),
            scope: engram.scope,
        })
    }

    fn open_engram_target(
        &self,
        config: &SyncConfig,
    ) -> Result<ProductionEngramTarget, DomainFailure> {
        let engram = config
            .engram
            .as_ref()
            .ok_or(DomainFailure::OperationFailed)?;
        match config
            .target_transport
            .as_ref()
            .unwrap_or(&SyncTargetTransport::Local)
        {
            SyncTargetTransport::Local => Ok(ProductionEngramTarget::Local {
                filesystem: LocalTargetFilesystem::open(&config.target_root, RootAccess::ReadWrite)
                    .map_err(|_| DomainFailure::OperationFailed)?,
                executable: engram
                    .executable
                    .clone()
                    .ok_or(DomainFailure::OperationFailed)?,
                project: config.target_root.join(engram.project_root.as_str()),
                project_id: engram.project_id.clone(),
            }),
            transport @ SyncTargetTransport::Ssh { root_id, .. } => {
                Ok(ProductionEngramTarget::Ssh {
                    filesystem: SshTargetFilesystem::new(
                        root_id.clone(),
                        self.ssh_factory.open(&production_ssh_target(transport)?)?,
                    ),
                    project_id: engram.project_id.clone(),
                    project_root: engram.project_root.clone(),
                })
            }
        }
    }

    fn persist_engram_receipt(
        &self,
        receipt: &ProductionEngramReceipt,
    ) -> Result<Value, DomainFailure> {
        let bytes =
            serde_json::to_vec_pretty(receipt).map_err(|_| DomainFailure::OperationFailed)?;
        write_private_atomic(
            &self
                .receipt_root
                .join("engram")
                .join(format!("{}.json", receipt.run_id)),
            &bytes,
        )?;
        serde_json::to_value(receipt).map_err(|_| DomainFailure::OperationFailed)
    }

    fn authority_path(&self, plan: &Plan) -> PathBuf {
        self.config
            .adapter_state
            .join("provider-authority")
            .join(format!(
                "{}.json",
                plan.id.as_str().trim_start_matches("sha256:")
            ))
    }

    fn executable_digests(&self) -> Result<BTreeMap<PathBuf, Sha256Digest>, DomainFailure> {
        let mut digests = BTreeMap::new();
        if let Some(pipeline) = &self.config.provider_pipeline {
            for provider in &pipeline.providers {
                let executable = match provider {
                    ConfiguredProvider::Apm { executable, .. }
                    | ConfiguredProvider::Chezmoi { executable, .. } => Some(executable),
                    ConfiguredProvider::Native { .. } => None,
                };
                if let Some(path) = executable {
                    let bytes = fs::read(path).map_err(|_| DomainFailure::OperationFailed)?;
                    digests.insert(path.clone(), digest_bytes(&bytes)?);
                }
            }
        }
        Ok(digests)
    }

    fn provider_configuration_digest(&self) -> Result<Sha256Digest, DomainFailure> {
        let providers = self
            .config
            .provider_pipeline
            .as_ref()
            .map(|pipeline| {
                pipeline
                    .providers
                    .iter()
                    .map(|provider| match provider {
                        ConfiguredProvider::Native { version, files } => serde_json::json!({
                            "provider":"native", "version":version, "files":files.iter().map(|file| {
                                serde_json::json!({"path":file.path,"source":file.source})
                            }).collect::<Vec<_>>()
                        }),
                        ConfiguredProvider::Apm { executable, version, manifest, lockfile, policy, targets, managed_root } => serde_json::json!({
                            "provider":"apm", "executable":executable, "version":version,
                            "manifest":manifest, "lockfile":lockfile, "policy":policy,
                            "targets":targets, "managedRoot":managed_root
                        }),
                        ConfiguredProvider::Chezmoi { executable, source, config } => serde_json::json!({
                            "provider":"chezmoi", "executable":executable,
                            "version":commonkit_adapters::TESTED_CHEZMOI_VERSION,
                            "source":source, "config":config
                        }),
                    })
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        digest_domain_json(
            "commonkit.provider-execution-configuration.v1",
            &serde_json::json!({
                "targetId":self.config.target_id,
                "targetRoot":self.config.target_root,
                "targetTransport":self.config.target_transport,
                "targetPlatform":self.config.target_platform,
                "declaredRoots":self.config.declared_roots,
                "protectedRoots":self.config.protected_roots,
                "caseSensitive":self.config.case_sensitive,
                "targetIdentityDigest":self.config.target_identity_digest,
                "composedLoadoutDigest":self.config.composed_loadout_digest,
                "policyDigest":self.config.policy_digest,
                "relayClientRoot":self.config.relay_client_root,
                "styleguide":self.config.styleguide,
                "sourceRevision":self.config.provider_pipeline.as_ref().map(|pipeline| &pipeline.source.revision),
                "providers":providers,
            }),
        )
        .map_err(|_| DomainFailure::OperationFailed)
    }

    fn provider_input_digests(&self) -> Result<BTreeMap<PathBuf, Sha256Digest>, DomainFailure> {
        let mut digests = BTreeMap::new();
        if let Some(pipeline) = &self.config.provider_pipeline {
            let mut paths = Vec::new();
            for provider in &pipeline.providers {
                match provider {
                    ConfiguredProvider::Native { files, .. } => {
                        paths.extend(files.iter().map(|file| file.source.to_string()));
                    }
                    ConfiguredProvider::Apm {
                        manifest,
                        lockfile,
                        policy,
                        ..
                    } => {
                        paths.extend([
                            path_text(manifest)?.to_owned(),
                            path_text(lockfile)?.to_owned(),
                            path_text(policy)?.to_owned(),
                        ]);
                    }
                    ConfiguredProvider::Chezmoi { source, config, .. } => {
                        paths
                            .extend([path_text(source)?.to_owned(), path_text(config)?.to_owned()]);
                    }
                }
            }
            for relative in paths {
                let path = checked_repository_path(&pipeline.source.repository, &relative)?;
                collect_local_input_digests(&pipeline.source.repository, &path, &mut digests)?;
            }
        }
        if let Some(styleguide) = &self.config.styleguide {
            for input in styleguide
                .descriptors
                .iter()
                .chain([&styleguide.manifest, &styleguide.lockfile])
            {
                let path = self.styleguide_input_path(input)?;
                digests.insert(
                    input.clone(),
                    digest_bytes(&fs::read(path).map_err(|_| DomainFailure::OperationFailed)?)?,
                );
            }
        }
        Ok(digests)
    }

    fn styleguide_input_path(&self, path: &Path) -> Result<PathBuf, DomainFailure> {
        if let Some(pipeline) = &self.config.provider_pipeline {
            return checked_repository_path(&pipeline.source.repository, path_text(path)?);
        }
        if !path.is_absolute() {
            return Err(DomainFailure::OperationFailed);
        }
        let metadata = fs::symlink_metadata(path).map_err(|_| DomainFailure::OperationFailed)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(DomainFailure::OperationFailed);
        }
        path.canonicalize()
            .map_err(|_| DomainFailure::OperationFailed)
    }

    fn bind_styleguide(
        &self,
        states: &[MaterializedState],
    ) -> Result<Option<MaterializedState>, DomainFailure> {
        let Some(config) = &self.config.styleguide else {
            return Ok(None);
        };
        let mut descriptors = BTreeMap::new();
        for path in &config.descriptors {
            let path = self.styleguide_input_path(path)?;
            let descriptor: StyleguideDescriptor = serde_json::from_slice(
                &fs::read(path).map_err(|_| DomainFailure::OperationFailed)?,
            )
            .map_err(|error| {
                eprintln!("commonkitd: styleguide descriptor parse failed: {error}");
                DomainFailure::OperationFailed
            })?;
            descriptor.validate().map_err(|error| {
                eprintln!("commonkitd: styleguide descriptor validation failed: {error}");
                DomainFailure::OperationFailed
            })?;
            if descriptors
                .insert(descriptor.exported_skill.clone(), descriptor)
                .is_some()
            {
                return Err(DomainFailure::OperationFailed);
            }
        }
        let staged_exports = descriptors
            .keys()
            .filter(|skill_id| {
                let suffix = format!("/skills/{}/SKILL.md", skill_id.as_str());
                let source_suffix = format!(".apm/skills/{}/SKILL.md", skill_id.as_str());
                states.iter().any(|state| {
                    state.resources.iter().any(|resource| {
                        resource.provenance.source.ends_with(&source_suffix)
                            && matches!(
                                &resource.intent,
                                FilesystemIntent::File { path, .. }
                                    if path.as_str().ends_with(&suffix)
                            )
                    })
                })
            })
            .cloned()
            .collect::<BTreeSet<_>>();
        let policy = StyleguidePolicy {
            denied: config.policy.denied,
            pinned_skill_id: config.policy.pinned_skill_id.clone(),
        };
        let resolved = resolve_styleguide_selection(
            Some(&config.selection),
            &descriptors,
            &staged_exports,
            &policy,
        )
        .map_err(|error| {
            eprintln!("commonkitd: styleguide admission failed: {error}");
            DomainFailure::OperationFailed
        })?
        .ok_or(DomainFailure::OperationFailed)?;
        let manifest_digest = digest_bytes(
            &fs::read(self.styleguide_input_path(&config.manifest)?)
                .map_err(|_| DomainFailure::OperationFailed)?,
        )?;
        let lock_digest = digest_bytes(
            &fs::read(self.styleguide_input_path(&config.lockfile)?)
                .map_err(|_| DomainFailure::OperationFailed)?,
        )?;
        if resolved.descriptor.package.manifest_digest != manifest_digest
            || resolved.descriptor.package.lock_digest != lock_digest
        {
            eprintln!(
                "commonkitd: styleguide APM manifest or lock digest does not match the descriptor"
            );
            return Err(DomainFailure::OperationFailed);
        }
        let binding_digest = resolved
            .binding
            .digest()
            .map_err(|_| DomainFailure::OperationFailed)?;
        let inputs = ProviderInputs::new(
            StableId::parse("commonkit-styleguide").map_err(|_| DomainFailure::OperationFailed)?,
            ExactProviderVersion::parse(&resolved.descriptor.package.version)
                .map_err(|_| DomainFailure::OperationFailed)?,
            "commonkit.styleguide-binding.v1".into(),
            BTreeMap::from([
                ("binding".into(), binding_digest),
                ("manifestFile".into(), manifest_digest),
                ("lockFile".into(), lock_digest),
            ]),
            vec!["styleguide".into()],
        )
        .map_err(|_| DomainFailure::OperationFailed)?;
        MaterializedState::finalize(inputs, Vec::new(), Vec::new(), Vec::new())
            .map(Some)
            .map_err(|_| DomainFailure::OperationFailed)
    }

    fn external_state_digests(&self) -> Result<BTreeMap<PathBuf, Sha256Digest>, DomainFailure> {
        self.config
            .materialized_states
            .iter()
            .map(|path| {
                let bytes = fs::read(path).map_err(|_| DomainFailure::OperationFailed)?;
                Ok((path.clone(), digest_bytes(&bytes)?))
            })
            .collect()
    }

    fn seal_execution_authority(
        &self,
        plan: &Plan,
        states: &[MaterializedState],
    ) -> Result<(), DomainFailure> {
        let relay_endpoint = states
            .iter()
            .any(|state| !state.capabilities.is_empty())
            .then(|| self.config.relay_endpoint.clone())
            .flatten();
        let record = ProviderAuthorityRecord {
            plan: plan.clone(),
            states: states.to_vec(),
            relay_endpoint,
            executable_digests: self.executable_digests()?,
            external_state_digests: self.external_state_digests()?,
            provider_input_digests: self.provider_input_digests()?,
            provider_configuration_digest: self.provider_configuration_digest()?,
            source_revision: self
                .config
                .provider_pipeline
                .as_ref()
                .map(|pipeline| pipeline.source.revision.clone()),
        };
        let path = self.authority_path(plan);
        if path.exists() {
            return if self.load_execution_authority(plan)? == record {
                Ok(())
            } else {
                Err(DomainFailure::OperationFailed)
            };
        }
        let key = self.load_or_create_authority_key()?;
        let cipher = XChaCha20Cipher::new(key);
        let plaintext = serde_json::to_vec(&record).map_err(|_| DomainFailure::OperationFailed)?;
        let ciphertext = cipher
            .seal(&plaintext, plan.id.as_str().as_bytes())
            .map_err(|_| DomainFailure::OperationFailed)?;
        let envelope = AuthenticatedProviderAuthority {
            schema: "commonkit.provider-authority.v1".into(),
            algorithm: cipher.algorithm().into(),
            ciphertext,
        };
        let bytes = serde_json::to_vec(&envelope).map_err(|_| DomainFailure::OperationFailed)?;
        let parent = path.parent().ok_or(DomainFailure::OperationFailed)?;
        fs::create_dir_all(parent).map_err(|_| DomainFailure::OperationFailed)?;
        let temporary = path.with_extension(format!("tmp-{}", std::process::id()));
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary)
            .map_err(|_| DomainFailure::OperationFailed)?;
        file.write_all(&bytes)
            .and_then(|_| file.sync_all())
            .map_err(|_| DomainFailure::OperationFailed)?;
        fs::rename(&temporary, &path).map_err(|_| DomainFailure::OperationFailed)?;
        sync_parent_directory(parent)
    }

    fn load_execution_authority(
        &self,
        plan: &Plan,
    ) -> Result<ProviderAuthorityRecord, DomainFailure> {
        let path = self.authority_path(plan);
        let metadata = fs::symlink_metadata(&path).map_err(|_| DomainFailure::OperationFailed)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(DomainFailure::OperationFailed);
        }
        let bytes = fs::read(path).map_err(|_| DomainFailure::OperationFailed)?;
        let envelope: AuthenticatedProviderAuthority =
            serde_json::from_slice(&bytes).map_err(|_| DomainFailure::OperationFailed)?;
        if envelope.schema != "commonkit.provider-authority.v1"
            || envelope.algorithm != "XCHACHA20-POLY1305"
        {
            return Err(DomainFailure::OperationFailed);
        }
        let key = self.load_authority_key()?;
        let plaintext = XChaCha20Cipher::new(key)
            .open(&envelope.ciphertext, plan.id.as_str().as_bytes())
            .map_err(|_| DomainFailure::OperationFailed)?;
        let record: ProviderAuthorityRecord =
            serde_json::from_slice(&plaintext).map_err(|_| DomainFailure::OperationFailed)?;
        if record.plan.id != plan.id || record.plan.target_id != plan.target_id {
            return Err(DomainFailure::OperationFailed);
        }
        Ok(record)
    }

    fn authority_key_path(&self) -> PathBuf {
        self.config.adapter_state.join("provider-authority.key")
    }

    fn load_authority_key(&self) -> Result<[u8; 32], DomainFailure> {
        let path = self.authority_key_path();
        let metadata = fs::symlink_metadata(&path).map_err(|_| DomainFailure::OperationFailed)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(DomainFailure::OperationFailed);
        }
        let bytes = fs::read(path).map_err(|_| DomainFailure::OperationFailed)?;
        bytes.try_into().map_err(|_| DomainFailure::OperationFailed)
    }

    fn load_or_create_authority_key(&self) -> Result<[u8; 32], DomainFailure> {
        if self.authority_key_path().exists() {
            return self.load_authority_key();
        }
        let mut key = [0_u8; 32];
        rand::rng().fill_bytes(&mut key);
        write_private_atomic(&self.authority_key_path(), &key)?;
        Ok(key)
    }

    fn states(&self) -> Result<Vec<MaterializedState>, DomainFailure> {
        if let Some(config) = &self.config.provider_pipeline {
            return self.materialize_configured(config);
        }
        self.config
            .materialized_states
            .iter()
            .map(|path| {
                let metadata =
                    fs::symlink_metadata(path).map_err(|_| DomainFailure::OperationFailed)?;
                if !metadata.is_file() || metadata.file_type().is_symlink() {
                    return Err(DomainFailure::OperationFailed);
                }
                serde_json::from_slice(&fs::read(path).map_err(|_| DomainFailure::OperationFailed)?)
                    .map_err(|_| DomainFailure::OperationFailed)
            })
            .collect()
    }

    fn materialize_configured(
        &self,
        config: &ProviderPipelineConfig,
    ) -> Result<Vec<MaterializedState>, DomainFailure> {
        let mut repository = GitRepository::new(
            ProcessGitRunner::new(&config.source.repository),
            config.source.trusted_remote_url.clone(),
            "origin",
        );
        let status = repository.inspect(false).map_err(|error| {
            eprintln!("commonkitd: local provider repository inspection failed: {error}");
            DomainFailure::OperationFailed
        })?;
        if status.revision.as_str() != config.source.revision
            || matches!(
                status.disposition,
                GitSyncDisposition::Dirty
                    | GitSyncDisposition::Behind
                    | GitSyncDisposition::Diverged
            )
        {
            eprintln!(
                "commonkitd: provider repository {} is not at the configured revision: \
                 configured {}, head {}, disposition {:?}",
                config.source.repository.display(),
                config.source.revision,
                status.revision.as_str(),
                status.disposition
            );
            return Err(DomainFailure::OperationFailed);
        }
        let artifacts = ArtifactStore::open(&self.config.provider_artifacts)
            .map_err(|_| DomainFailure::OperationFailed)?;
        let rules = OwnershipRules::new(
            self.config.case_sensitive,
            self.config.declared_roots.clone(),
            self.config.protected_roots.clone(),
        )
        .map_err(|_| DomainFailure::OperationFailed)?;
        let pipeline = ProviderPipeline::open(
            &config.root,
            artifacts,
            rules,
            vec![
                self.config.target_root.clone(),
                self.config.adapter_state.clone(),
                config.source.repository.clone(),
            ],
        )
        .map_err(|_| DomainFailure::OperationFailed)?;
        let target_platform = self.config.provider_platform()?;
        let context = ProviderContext {
            target_id: self.config.target_id.clone(),
            platform: target_platform.operating_system,
            architecture: target_platform.architecture,
            policy_digest: self.config.policy_digest.clone(),
            declared_roots: self.config.declared_roots.clone(),
            observed_fact_digests: BTreeMap::new(),
        };
        let mut providers: Vec<Box<dyn DesiredStateProvider>> = Vec::new();
        for provider in &config.providers {
            providers.push(self.configured_provider(provider, config, pipeline.artifacts())?);
        }
        let references = providers
            .iter()
            .map(|provider| provider.as_ref() as &dyn DesiredStateProvider)
            .collect::<Vec<_>>();
        pipeline
            .materialize_all(&references, &context)
            .map(|result| result.states)
            .map_err(|error| {
                eprintln!("commonkitd: provider materialization failed: {error}");
                DomainFailure::OperationFailed
            })
    }

    fn configured_provider(
        &self,
        config: &ConfiguredProvider,
        pipeline: &ProviderPipelineConfig,
        artifacts: &ArtifactStore,
    ) -> Result<Box<dyn DesiredStateProvider>, DomainFailure> {
        match config {
            ConfiguredProvider::Native { version, files } => {
                let mut input_digests = BTreeMap::from([(
                    "repositoryRevision".into(),
                    digest_text(
                        "commonkit.git-provider-revision.v1",
                        &pipeline.source.revision,
                    )?,
                )]);
                let mut staged = Vec::new();
                for file in files {
                    let source =
                        checked_repository_path(&pipeline.source.repository, file.source.as_str())?;
                    let bytes = fs::read(&source).map_err(|_| DomainFailure::OperationFailed)?;
                    input_digests.insert(
                        format!("source:{}", file.source.as_str()),
                        digest_bytes(&bytes)?,
                    );
                    let content = artifacts
                        .put(&bytes, ContentSensitivity::Portable)
                        .map_err(|_| DomainFailure::OperationFailed)?;
                    staged.push((file, content));
                }
                let inputs = ProviderInputs::new(
                    StableId::parse("native").map_err(|_| DomainFailure::OperationFailed)?,
                    version.clone(),
                    "commonkit.native-provider.v1".into(),
                    input_digests,
                    vec!["files".into()],
                )
                .map_err(|_| DomainFailure::OperationFailed)?;
                let resources = staged
                    .into_iter()
                    .map(|(file, content)| NormalizedResource {
                        intent: FilesystemIntent::File {
                            path: file.path.clone(),
                            content,
                            mode: None,
                            expected_before: None,
                        },
                        provenance: ResourceProvenance {
                            provider_id: inputs.provider_id.clone(),
                            provider_version: inputs.provider_version.to_string(),
                            input_digest: inputs.input_set_digest.clone(),
                            source: file.source.to_string(),
                        },
                    })
                    .collect();
                NativeProvider::new(inputs, resources)
                    .map(|provider| Box::new(provider) as Box<dyn DesiredStateProvider>)
                    .map_err(|_| DomainFailure::OperationFailed)
            }
            ConfiguredProvider::Apm {
                executable,
                version,
                manifest,
                lockfile,
                policy,
                targets,
                managed_root,
            } => ApmProvider::new(ApmProviderConfig {
                executable: executable.clone(),
                git_executable: None,
                version: version.clone(),
                manifest: checked_repository_path(
                    &pipeline.source.repository,
                    path_text(manifest)?,
                )?,
                lockfile: checked_repository_path(
                    &pipeline.source.repository,
                    path_text(lockfile)?,
                )?,
                policy: checked_repository_path(&pipeline.source.repository, path_text(policy)?)?,
                targets: targets.clone(),
                managed_root: managed_root.clone(),
                bound_source: None,
            })
            .map(|provider| Box::new(provider) as Box<dyn DesiredStateProvider>)
            .map_err(|_| DomainFailure::OperationFailed),
            ConfiguredProvider::Chezmoi {
                executable,
                source,
                config,
            } => {
                let workspace = pipeline.root.join("workspaces/chezmoi");
                ChezmoiProvider::new(
                    executable,
                    checked_repository_path(&pipeline.source.repository, path_text(source)?)?,
                    checked_repository_path(&pipeline.source.repository, path_text(config)?)?,
                    workspace.join("cache"),
                    workspace.join("state/chezmoi.db"),
                    workspace.join("working-tree"),
                )
                .map(|provider| Box::new(provider) as Box<dyn DesiredStateProvider>)
                .map_err(|_| DomainFailure::OperationFailed)
            }
        }
    }
    fn states_for_plan(
        &self,
        artifacts: &ArtifactStore,
    ) -> Result<Vec<MaterializedState>, DomainFailure> {
        let mut states = self.states()?;
        if let Some(binding) = self.bind_styleguide(&states)? {
            states.push(binding);
        }
        let has_mcp = states.iter().any(|state| !state.capabilities.is_empty());
        if has_mcp {
            if matches!(
                self.config.target_transport,
                Some(SyncTargetTransport::Ssh { .. })
            ) {
                // A loopback URL is target-local. An SSH controller cannot claim
                // it operates a relay on the remote host; run commonkitd on the
                // target until a transactional remote service adapter exists.
                return Err(DomainFailure::RelayRequiresTargetResidentDaemon);
            }
            let managed_root = self
                .config
                .relay_client_root
                .as_ref()
                .ok_or(DomainFailure::OperationFailed)?;
            if let Some(client_state) = materialize_mcp_client_state(
                &states,
                artifacts,
                managed_root,
                self.config
                    .relay_endpoint
                    .as_deref()
                    .unwrap_or("http://127.0.0.1:3764/mcp"),
            )
            .map_err(|_| DomainFailure::OperationFailed)?
            {
                states.push(client_state);
            }
        }
        Ok(states)
    }

    fn build(&self) -> Result<Plan, DomainFailure> {
        fs::create_dir_all(&self.config.adapter_state).map_err(|error| {
            eprintln!("commonkitd: adapter state preparation failed: {error}");
            DomainFailure::OperationFailed
        })?;
        let artifacts = ArtifactStore::open(&self.config.provider_artifacts).map_err(|error| {
            eprintln!("commonkitd: provider artifact store failed: {error}");
            DomainFailure::OperationFailed
        })?;
        let rules = OwnershipRules::new(
            self.config.case_sensitive,
            self.config.declared_roots.clone(),
            self.config.protected_roots.clone(),
        )
        .map_err(|error| {
            eprintln!("commonkitd: ownership policy preparation failed: {error}");
            DomainFailure::OperationFailed
        })?;
        let states = self.states_for_plan(&artifacts).inspect_err(|error| {
            eprintln!("commonkitd: desired-state preparation failed: {error:?}");
        })?;
        let resources = states
            .iter()
            .flat_map(|state| state.resources.iter().cloned())
            .collect::<Vec<_>>();
        validate_ownership(&resources, &rules).map_err(|error| {
            eprintln!("commonkitd: ownership validation failed: {error}");
            DomainFailure::OperationFailed
        })?;
        let intents = || {
            states
                .iter()
                .flat_map(|state| state.resources.iter().map(|resource| &resource.intent))
        };
        let plan = match self
            .config
            .target_transport
            .as_ref()
            .unwrap_or(&SyncTargetTransport::Local)
        {
            SyncTargetTransport::Local => {
                let mut files =
                    FileAdapter::open(&self.config.target_root, &self.config.adapter_state)
                        .map_err(|error| {
                            eprintln!("commonkitd: filesystem adapter preparation failed: {error}");
                            DomainFailure::OperationFailed
                        })?;
                let observed = files.observed_state_digest(intents()).map_err(|error| {
                    eprintln!("commonkitd: observed-state inspection failed: {error}");
                    DomainFailure::OperationFailed
                })?;
                self.finish_plan(&states, &artifacts, &rules, observed, &mut files)?
            }
            transport @ SyncTargetTransport::Ssh { root_id, .. } => {
                let target = production_ssh_target(transport)?;
                let mut ssh = self.ssh_factory.open(&target)?;
                for state in &states {
                    let suffix = &state.digest.as_str()[7..39];
                    RemoteProviderStager::new(&mut ssh)
                        .stage_materialized(
                            state,
                            &artifacts,
                            self.config.target_id.clone(),
                            StableId::parse(format!("provider-stage-{suffix}"))
                                .map_err(|_| DomainFailure::OperationFailed)?,
                        )
                        .map_err(|_| DomainFailure::OperationFailed)?;
                }
                let mut files =
                    SshFileAdapter::open(root_id.clone(), &self.config.adapter_state, ssh)
                        .map_err(|_| DomainFailure::OperationFailed)?;
                let observed = files
                    .observed_state_digest(intents())
                    .map_err(|_| DomainFailure::OperationFailed)?;
                self.finish_plan(&states, &artifacts, &rules, observed, &mut files)?
            }
        };
        self.seal_execution_authority(&plan, &states)
            .inspect_err(|error| {
                eprintln!("commonkitd: plan authority sealing failed: {error:?}");
            })?;
        self.plan_store.persist(&plan).map_err(|error| {
            eprintln!("commonkitd: plan persistence failed: {error}");
            DomainFailure::OperationFailed
        })?;
        Ok(plan)
    }

    fn finish_plan<P: ProviderResourcePlanner + ?Sized>(
        &self,
        states: &[MaterializedState],
        artifacts: &ArtifactStore,
        rules: &OwnershipRules,
        observed_digest: Sha256Digest,
        files: &mut P,
    ) -> Result<Plan, DomainFailure> {
        let plan = build_provider_plan(
            ProviderPlanRequest {
                target_id: self.config.target_id.clone(),
                target_identity_digest: self.config.target_identity_digest.clone(),
                composed_loadout_digest: self.config.composed_loadout_digest.clone(),
                observed_digest,
                policy_digest: self.config.policy_digest.clone(),
                ownership_rules: rules,
                mapped_side_effects: BTreeSet::new(),
            },
            states,
            artifacts,
            files,
        )
        .map_err(|_| DomainFailure::OperationFailed)?;
        Ok(plan)
    }
}

fn production_ssh_target(
    config: &SyncTargetTransport,
) -> Result<ProductionSshTarget, DomainFailure> {
    let SyncTargetTransport::Ssh {
        root_id,
        host,
        user,
        port,
        known_hosts,
        fingerprint,
    } = config
    else {
        return Err(DomainFailure::OperationFailed);
    };
    Ok(ProductionSshTarget {
        root_id: root_id.clone(),
        host: host.clone(),
        user: user.clone(),
        port: *port,
        known_hosts: known_hosts.clone(),
        fingerprint: fingerprint.clone(),
    })
}

fn path_text(path: &Path) -> Result<&str, DomainFailure> {
    path.to_str().ok_or(DomainFailure::OperationFailed)
}

fn checked_repository_path(repository: &Path, relative: &str) -> Result<PathBuf, DomainFailure> {
    let relative =
        NormalizedManagedPath::parse(relative).map_err(|_| DomainFailure::OperationFailed)?;
    let repository = repository
        .canonicalize()
        .map_err(|_| DomainFailure::OperationFailed)?;
    let path = repository.join(relative.as_str());
    let canonical = path
        .canonicalize()
        .map_err(|_| DomainFailure::OperationFailed)?;
    if !canonical.starts_with(&repository) {
        return Err(DomainFailure::OperationFailed);
    }
    Ok(canonical)
}

fn digest_text(domain: &str, value: &str) -> Result<Sha256Digest, DomainFailure> {
    digest_domain_json(domain, &value).map_err(|_| DomainFailure::OperationFailed)
}

fn digest_bytes(bytes: &[u8]) -> Result<Sha256Digest, DomainFailure> {
    Sha256Digest::parse(format!("sha256:{:x}", Sha256::digest(bytes)))
        .map_err(|_| DomainFailure::OperationFailed)
}

fn hmac_sha256(key: &[u8; 32], message: &[u8]) -> String {
    let mut inner_pad = [0x36_u8; 64];
    let mut outer_pad = [0x5c_u8; 64];
    for (index, byte) in key.iter().enumerate() {
        inner_pad[index] ^= byte;
        outer_pad[index] ^= byte;
    }
    let mut inner = Sha256::new();
    inner.update(inner_pad);
    inner.update(message);
    let inner_digest = inner.finalize();
    let mut outer = Sha256::new();
    outer.update(outer_pad);
    outer.update(inner_digest);
    format!("{:x}", outer.finalize())
}

fn collect_local_input_digests(
    repository: &Path,
    path: &Path,
    output: &mut BTreeMap<PathBuf, Sha256Digest>,
) -> Result<(), DomainFailure> {
    let repository = repository
        .canonicalize()
        .map_err(|_| DomainFailure::OperationFailed)?;
    let metadata = fs::symlink_metadata(path).map_err(|_| DomainFailure::OperationFailed)?;
    if metadata.file_type().is_symlink() || !path.starts_with(&repository) {
        return Err(DomainFailure::OperationFailed);
    }
    if metadata.is_file() {
        output.insert(
            path.strip_prefix(repository)
                .map_err(|_| DomainFailure::OperationFailed)?
                .to_path_buf(),
            digest_bytes(&fs::read(path).map_err(|_| DomainFailure::OperationFailed)?)?,
        );
        return Ok(());
    }
    if !metadata.is_dir() {
        return Err(DomainFailure::OperationFailed);
    }
    let mut children = fs::read_dir(path)
        .map_err(|_| DomainFailure::OperationFailed)?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|_| DomainFailure::OperationFailed)?;
    children.sort_by_key(|entry| entry.file_name());
    for child in children {
        collect_local_input_digests(&repository, &child.path(), output)?;
    }
    Ok(())
}

fn write_private_atomic(path: &Path, bytes: &[u8]) -> Result<(), DomainFailure> {
    let parent = path.parent().ok_or(DomainFailure::OperationFailed)?;
    fs::create_dir_all(parent).map_err(|_| DomainFailure::OperationFailed)?;
    write_private_atomic_prepared(path, bytes, &sync_directory_io)
}

fn write_private_atomic_with_sync(
    path: &Path,
    bytes: &[u8],
    directory_sync: &CredentialDirectorySync,
) -> Result<(), DomainFailure> {
    ensure_private_parent_with_sync(path, directory_sync)?;
    write_private_atomic_prepared(path, bytes, directory_sync)
}

fn write_private_atomic_prepared(
    path: &Path,
    bytes: &[u8],
    directory_sync: &CredentialDirectorySync,
) -> Result<(), DomainFailure> {
    let parent = path.parent().ok_or(DomainFailure::OperationFailed)?;
    let temporary = path.with_extension(format!("tmp-{:032x}", rand::random::<u128>()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options
        .open(&temporary)
        .map_err(|_| DomainFailure::OperationFailed)?;
    if file.write_all(bytes).and_then(|_| file.sync_all()).is_err() {
        let _ = fs::remove_file(&temporary);
        return Err(DomainFailure::OperationFailed);
    }
    fs::rename(&temporary, path).map_err(|_| DomainFailure::OperationFailed)?;
    sync_parent_directory_with(parent, directory_sync)
}

fn sync_parent_directory(parent: &Path) -> Result<(), DomainFailure> {
    sync_parent_directory_with(parent, &sync_directory_io)
}

fn sync_path_parent_with(
    path: &Path,
    directory_sync: &CredentialDirectorySync,
) -> Result<(), DomainFailure> {
    let parent = path.parent().ok_or(DomainFailure::OperationFailed)?;
    sync_parent_directory_with(parent, directory_sync)
}

fn remove_file_with_sync(
    path: &Path,
    directory_sync: &CredentialDirectorySync,
) -> Result<(), DomainFailure> {
    match fs::remove_file(path) {
        Ok(()) => sync_path_parent_with(path, directory_sync),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(_) => Err(DomainFailure::OperationFailed),
    }
}

fn sync_parent_directory_with(
    parent: &Path,
    directory_sync: &CredentialDirectorySync,
) -> Result<(), DomainFailure> {
    directory_sync(parent).map_err(|_| DomainFailure::OperationFailed)
}

#[cfg(unix)]
fn sync_directory_io(path: &Path) -> Result<(), std::io::Error> {
    fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_directory_io(path: &Path) -> Result<(), std::io::Error> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?
        .sync_all()
}

#[cfg(not(any(unix, windows)))]
fn sync_directory_io(path: &Path) -> Result<(), std::io::Error> {
    fs::File::open(path)?.sync_all()
}

fn relay_authority_from_states(
    config: &SyncConfig,
    states: Vec<MaterializedState>,
) -> Result<RelayProviderAuthority, DomainFailure> {
    let artifact_store = ArtifactStore::open_existing(&config.provider_artifacts)
        .map_err(|_| DomainFailure::OperationFailed)?;
    for state in &states {
        state.verify().map_err(|_| DomainFailure::OperationFailed)?;
        for resource in &state.resources {
            if let FilesystemIntent::File { content, .. } = &resource.intent {
                artifact_store
                    .load(content)
                    .map_err(|_| DomainFailure::OperationFailed)?;
            }
        }
    }
    let mut provider_inputs = states
        .iter()
        .map(|state| state.inputs.input_set_digest.clone())
        .collect::<Vec<_>>();
    provider_inputs.sort();
    let mut artifacts = states
        .iter()
        .map(|state| state.digest.clone())
        .collect::<Vec<_>>();
    artifacts.sort();
    let mut ownership = states
        .iter()
        .flat_map(|state| state.capabilities.iter())
        .map(|resource| {
            let ProviderCapability::McpStreamableHttp { id, .. } = &resource.capability;
            (
                id.clone(),
                resource.provenance.provider_id.clone(),
                resource.provenance.source.clone(),
            )
        })
        .collect::<Vec<_>>();
    ownership.sort();
    if ownership.windows(2).any(|pair| pair[0].0 == pair[1].0) {
        return Err(DomainFailure::OperationFailed);
    }
    Ok(RelayProviderAuthority {
        materialized_states: states,
        relay_is_target_local: matches!(
            config
                .target_transport
                .as_ref()
                .unwrap_or(&SyncTargetTransport::Local),
            SyncTargetTransport::Local
        ),
        target_identity_digest: config.target_identity_digest.clone(),
        composed_loadout_digest: config.composed_loadout_digest.clone(),
        provider_inputs_digest: digest_domain_json(
            "commonkit.relay-provider-inputs.v1",
            &provider_inputs,
        )
        .map_err(|_| DomainFailure::OperationFailed)?,
        policy_digest: config.policy_digest.clone(),
        ownership_map_digest: digest_domain_json(
            "commonkit.relay-capability-ownership.v1",
            &ownership,
        )
        .map_err(|_| DomainFailure::OperationFailed)?,
        artifact_set_digest: digest_domain_json("commonkit.relay-materialized-set.v1", &artifacts)
            .map_err(|_| DomainFailure::OperationFailed)?,
    })
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PlanRequest {
    confirmed: bool,
    #[serde(rename = "confirmationId")]
    confirmation_id: StableId,
    #[serde(rename = "idempotencyKey")]
    idempotency_key: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
struct VerifyRequest {
    plan_id: Option<Sha256Digest>,
    target_id: Option<StableId>,
    pointer: Option<String>,
}
impl VerifyRequest {
    fn acknowledge_metadata(&self) {
        let _ = (&self.target_id, &self.pointer);
    }
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RollbackRequest {
    run_id: StableId,
    confirmed: bool,
    confirmation_id: StableId,
    idempotency_key: Option<String>,
}
impl SyncDomain for ProductionSyncDomain {
    fn engram_status(&self) -> Result<Value, DomainFailure> {
        self.configured_engram_status()?
            .ok_or(DomainFailure::OperationFailed)
    }

    fn engram_reconcile(&self, request: Value) -> Result<Value, DomainFailure> {
        let request: EngramReconcileRequest =
            serde_json::from_value(request).map_err(|_| DomainFailure::InvalidRequest)?;
        if !request.confirmed || request.peer_target_id == self.config.target_id {
            return Err(DomainFailure::InvalidRequest);
        }
        let _guard = self
            .engram_lock
            .lock()
            .map_err(|_| DomainFailure::OperationFailed)?;
        let peer_config = self
            .sync_configs
            .get(&request.peer_target_id)
            .ok_or(DomainFailure::InvalidRequest)?;
        let left_declaration = Self::engram_declaration(&self.config)?;
        let right_declaration = Self::engram_declaration(peer_config)?;
        if left_declaration.scope != EngramScope::Project
            || right_declaration.scope != EngramScope::Project
            || left_declaration.project_id != right_declaration.project_id
        {
            return Err(DomainFailure::InvalidRequest);
        }
        let grant = if left_declaration.owner_id == right_declaration.owner_id {
            None
        } else {
            Some(
                self.engram_grants
                    .iter()
                    .find(|grant| {
                        grant.project_id == left_declaration.project_id
                            && ((grant.grantor == left_declaration.owner_id
                                && grant.grantee == right_declaration.owner_id)
                                || (grant.grantor == right_declaration.owner_id
                                    && grant.grantee == left_declaration.owner_id))
                    })
                    .ok_or(DomainFailure::OperationFailed)?,
            )
        };
        let run_id = StableId::parse(format!("engram-{:032x}", rand::random::<u128>()))
            .map_err(|_| DomainFailure::OperationFailed)?;
        if grant.is_some_and(|grant| grant.state == EngramGrantState::Withdrawn) {
            return self.persist_engram_receipt(&ProductionEngramReceipt {
                run_id,
                confirmation_id: request.confirmation_id,
                reconciliation: commonkit_adapters::EngramTargetReconciliationReceipt {
                    project_id: left_declaration.project_id,
                    moved: Vec::new(),
                    unresolved: Some(
                        "grant withdrawn; previously materialized observations are retained"
                            .to_owned(),
                    ),
                },
                imports_completed: false,
            });
        }
        let left_target = self.open_engram_target(&self.config)?;
        let right_target = self.open_engram_target(peer_config)?;
        left_target.sync(EngramTargetSyncMode::Export)?;
        if right_target.sync(EngramTargetSyncMode::Export).is_err() {
            return self.persist_engram_receipt(&ProductionEngramReceipt {
                run_id,
                confirmation_id: request.confirmation_id,
                reconciliation: commonkit_adapters::EngramTargetReconciliationReceipt {
                    project_id: left_declaration.project_id,
                    moved: Vec::new(),
                    unresolved: Some("peer target is unreachable".to_owned()),
                },
                imports_completed: false,
            });
        }
        let mut reconciliation = match grant {
            None => EngramChunkAdapter::reconcile_targets(
                left_target.filesystem(),
                &left_declaration,
                right_target.filesystem(),
                &right_declaration,
            ),
            Some(grant) if grant.grantor == left_declaration.owner_id => {
                EngramChunkAdapter::reconcile_granted_targets(
                    left_target.filesystem(),
                    &left_declaration,
                    right_target.filesystem(),
                    &right_declaration,
                    grant,
                )
            }
            Some(grant) => EngramChunkAdapter::reconcile_granted_targets(
                right_target.filesystem(),
                &right_declaration,
                left_target.filesystem(),
                &left_declaration,
                grant,
            ),
        }
        .map_err(|_| DomainFailure::OperationFailed)?;
        let imports_completed = left_target.sync(EngramTargetSyncMode::Import).is_ok()
            && right_target.sync(EngramTargetSyncMode::Import).is_ok();
        if !imports_completed {
            reconciliation.unresolved =
                Some("chunk exchange completed but import is unresolved".to_owned());
        }
        self.persist_engram_receipt(&ProductionEngramReceipt {
            run_id,
            confirmation_id: request.confirmation_id,
            reconciliation,
            imports_completed,
        })
    }

    fn git_sync(&self, fetch: bool) -> Result<Value, DomainFailure> {
        let source = self
            .config
            .provider_pipeline
            .as_ref()
            .map(|pipeline| &pipeline.source)
            .ok_or(DomainFailure::OperationFailed)?;
        let mut repository = GitRepository::new(
            ProcessGitRunner::new(&source.repository),
            source.trusted_remote_url.clone(),
            "origin",
        );
        let status = repository
            .inspect(fetch)
            .map_err(|_| DomainFailure::OperationFailed)?;
        let state = match status.disposition {
            GitSyncDisposition::Clean => "clean",
            GitSyncDisposition::Dirty => "dirty",
            GitSyncDisposition::Ahead => "ahead",
            GitSyncDisposition::Behind => "behind",
            GitSyncDisposition::Diverged => "diverged",
        };
        Ok(serde_json::json!({
            "state": state,
            "branch": status.branch,
            "revision": status.revision.as_str(),
            "upstreamRevision": status.upstream_revision.as_str(),
            "remoteUrl": status.remote_url,
            "fetched": fetch
        }))
    }
    fn persist_plan_execution_authority(
        &self,
        plan: &Plan,
        states: &[MaterializedState],
    ) -> Result<(), DomainFailure> {
        self.seal_execution_authority(plan, states)
    }

    fn plan_execution_authority(
        &self,
        approved: &Plan,
    ) -> Result<crate::PlanExecutionAuthority, DomainFailure> {
        let artifacts = ArtifactStore::open_existing(&self.config.provider_artifacts)
            .map_err(|_| DomainFailure::OperationFailed)?;
        let record = self.load_execution_authority(approved)?;
        let expected_relay_endpoint = record
            .states
            .iter()
            .any(|state| !state.capabilities.is_empty())
            .then(|| self.config.relay_endpoint.clone())
            .flatten();
        if record.relay_endpoint != expected_relay_endpoint
            || record.executable_digests != self.executable_digests()?
            || record.external_state_digests != self.external_state_digests()?
            || record.provider_input_digests != self.provider_input_digests()?
            || record.provider_configuration_digest != self.provider_configuration_digest()?
        {
            return Err(DomainFailure::OperationFailed);
        }
        if let Some(pipeline) = &self.config.provider_pipeline {
            if record.source_revision.as_deref() != Some(pipeline.source.revision.as_str()) {
                return Err(DomainFailure::OperationFailed);
            }
            for configured in &pipeline.providers {
                let expected = match configured {
                    ConfiguredProvider::Native { version, .. } => ("native", version.to_string()),
                    ConfiguredProvider::Apm { version, .. } => ("apm", version.to_string()),
                    ConfiguredProvider::Chezmoi { .. } => (
                        "chezmoi",
                        commonkit_adapters::TESTED_CHEZMOI_VERSION.to_owned(),
                    ),
                };
                if !record.states.iter().any(|state| {
                    state.inputs.provider_id.as_str() == expected.0
                        && state.inputs.provider_version.to_string() == expected.1
                }) {
                    return Err(DomainFailure::OperationFailed);
                }
            }
        }
        for state in &record.states {
            state.verify().map_err(|_| DomainFailure::OperationFailed)?;
            for resource in &state.resources {
                if let FilesystemIntent::File { content, .. } = &resource.intent {
                    artifacts
                        .load(content)
                        .map_err(|_| DomainFailure::OperationFailed)?;
                }
            }
        }
        let reconstructed = build_plan(PlanDraft {
            target_id: record.plan.target_id.clone(),
            desired_digest: record.plan.desired_digest.clone(),
            observed_digest: record.plan.observed_digest.clone(),
            policy_digest: record.plan.policy_digest.clone(),
            bindings: record.plan.bindings.clone(),
            operations: record.plan.operations.clone(),
        })
        .map_err(|_| DomainFailure::OperationFailed)?;
        if reconstructed != record.plan || reconstructed != *approved {
            return Err(DomainFailure::OperationFailed);
        }
        Ok(crate::PlanExecutionAuthority {
            policy_digest: reconstructed.policy_digest.clone(),
            bindings: reconstructed.bindings.clone(),
            plan_digest: reconstructed.id,
        })
    }

    fn relay_execution_authority(
        &self,
        approved: &Plan,
    ) -> Result<RelayProviderAuthority, DomainFailure> {
        self.plan_execution_authority(approved)?;
        let record = self.load_execution_authority(approved)?;
        relay_authority_from_states(&self.config, record.states)
    }

    fn relay_provider_authority(&self) -> Result<RelayProviderAuthority, DomainFailure> {
        let states = self.states()?;
        let artifact_store = ArtifactStore::open(&self.config.provider_artifacts)
            .map_err(|_| DomainFailure::OperationFailed)?;
        for state in &states {
            state.verify().map_err(|_| DomainFailure::OperationFailed)?;
            for resource in &state.resources {
                if let FilesystemIntent::File { content, .. } = &resource.intent {
                    artifact_store
                        .load(content)
                        .map_err(|_| DomainFailure::OperationFailed)?;
                }
            }
        }
        let mut provider_inputs = states
            .iter()
            .map(|state| state.inputs.input_set_digest.clone())
            .collect::<Vec<_>>();
        provider_inputs.sort();
        let mut artifacts = states
            .iter()
            .map(|state| state.digest.clone())
            .collect::<Vec<_>>();
        artifacts.sort();
        let mut ownership = states
            .iter()
            .flat_map(|state| state.capabilities.iter())
            .map(|resource| {
                let ProviderCapability::McpStreamableHttp { id, .. } = &resource.capability;
                (
                    id.clone(),
                    resource.provenance.provider_id.clone(),
                    resource.provenance.source.clone(),
                )
            })
            .collect::<Vec<_>>();
        ownership.sort();
        if ownership.windows(2).any(|pair| pair[0].0 == pair[1].0) {
            return Err(DomainFailure::OperationFailed);
        }
        Ok(RelayProviderAuthority {
            materialized_states: states,
            relay_is_target_local: matches!(
                self.config
                    .target_transport
                    .as_ref()
                    .unwrap_or(&SyncTargetTransport::Local),
                SyncTargetTransport::Local
            ),
            target_identity_digest: self.config.target_identity_digest.clone(),
            composed_loadout_digest: self.config.composed_loadout_digest.clone(),
            provider_inputs_digest: digest_domain_json(
                "commonkit.relay-provider-inputs.v1",
                &provider_inputs,
            )
            .map_err(|_| DomainFailure::OperationFailed)?,
            policy_digest: self.config.policy_digest.clone(),
            ownership_map_digest: digest_domain_json(
                "commonkit.relay-capability-ownership.v1",
                &ownership,
            )
            .map_err(|_| DomainFailure::OperationFailed)?,
            artifact_set_digest: digest_domain_json(
                "commonkit.relay-materialized-set.v1",
                &artifacts,
            )
            .map_err(|_| DomainFailure::OperationFailed)?,
        })
    }

    fn plan(&self, request: Value) -> Result<Value, DomainFailure> {
        let request: PlanRequest =
            serde_json::from_value(request).map_err(|_| DomainFailure::InvalidRequest)?;
        let _request_metadata = (
            request.confirmed,
            request.confirmation_id,
            request.idempotency_key,
        );
        serde_json::to_value(self.build()?).map_err(|_| DomainFailure::OperationFailed)
    }
    fn verify(&self, request: Value) -> Result<Value, DomainFailure> {
        let reference: VerifyRequest =
            serde_json::from_value(request).map_err(|_| DomainFailure::InvalidRequest)?;
        reference.acknowledge_metadata();
        let plan = match reference.plan_id {
            Some(id) => self
                .plan_store
                .load(&id)
                .map_err(|_| DomainFailure::InvalidRequest)?,
            None => self.build()?,
        };
        match self
            .config
            .target_transport
            .as_ref()
            .unwrap_or(&SyncTargetTransport::Local)
        {
            SyncTargetTransport::Local => {
                let mut adapter =
                    FileAdapter::open(&self.config.target_root, &self.config.adapter_state)
                        .map_err(|_| DomainFailure::OperationFailed)?;
                for operation in &plan.operations {
                    adapter
                        .verify(operation)
                        .map_err(|_| DomainFailure::VerificationFailed)?;
                }
            }
            transport @ SyncTargetTransport::Ssh { root_id, .. } => {
                let mut adapter = SshFileAdapter::open(
                    root_id.clone(),
                    &self.config.adapter_state,
                    self.ssh_factory.open(&production_ssh_target(transport)?)?,
                )
                .map_err(|_| DomainFailure::OperationFailed)?;
                for operation in &plan.operations {
                    adapter
                        .verify(operation)
                        .map_err(|_| DomainFailure::VerificationFailed)?;
                }
            }
        }
        let mut report = serde_json::json!({"planId":plan.id,"verified":true});
        if let Some(engram) = self.configured_engram_status()? {
            report["engram"] = engram;
        }
        Ok(report)
    }
    fn rollback(&self, request: Value) -> Result<Value, DomainFailure> {
        let reference: RollbackRequest =
            serde_json::from_value(request).map_err(|_| DomainFailure::InvalidRequest)?;
        let _metadata = (
            reference.confirmed,
            reference.confirmation_id,
            reference.idempotency_key,
        );
        let run_id = reference.run_id;
        let receipts =
            ReceiptStore::open(&self.receipt_root).map_err(|_| DomainFailure::OperationFailed)?;
        let receipt = receipts
            .load(run_id.clone())
            .map_err(|_| DomainFailure::InvalidRequest)?;
        let plan = self
            .plan_store
            .load(&receipt.receipt().plan_id)
            .map_err(|_| DomainFailure::OperationFailed)?;
        let mut adapters: Vec<Box<dyn Adapter>> = match self
            .config
            .target_transport
            .as_ref()
            .unwrap_or(&SyncTargetTransport::Local)
        {
            SyncTargetTransport::Local => vec![Box::new(
                FileAdapter::open(&self.config.target_root, &self.config.adapter_state)
                    .map_err(|_| DomainFailure::OperationFailed)?,
            )],
            transport @ SyncTargetTransport::Ssh { root_id, .. } => vec![Box::new(
                SshFileAdapter::open(
                    root_id.clone(),
                    &self.config.adapter_state,
                    self.ssh_factory.open(&production_ssh_target(transport)?)?,
                )
                .map_err(|_| DomainFailure::OperationFailed)?,
            )],
        };
        let outcome = Reconciler::with_store(&receipts)
            .rollback_succeeded_run(run_id.clone(), &plan, &mut adapters)
            .map_err(|_| DomainFailure::OperationFailed)?;
        Ok(serde_json::json!({"runId":run_id,"outcome":format!("{outcome:?}").to_lowercase()}))
    }
}

struct ProductionCredentialDomain {
    root: PathBuf,
    bws_executable: Option<PathBuf>,
    destinations: BTreeMap<StableId, CredentialDestination>,
    lock: Mutex<()>,
    directory_sync: Arc<CredentialDirectorySync>,
}
type CredentialDirectorySync = dyn Fn(&Path) -> Result<(), std::io::Error> + Send + Sync;
const CREDENTIAL_STATE_DIRECTORY: &str = ".commonkit-credentials";

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct StoredCredentialPlan {
    schema: String,
    plan_id: Sha256Digest,
    destinations: Vec<PlannedCredentialDestination>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PlannedCredentialDestination {
    destination_id: StableId,
    path: NormalizedManagedPath,
    reference_scheme: String,
    reference_digest: Sha256Digest,
    observed_digest: Sha256Digest,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CredentialReceipt {
    schema: String,
    receipt_id: Sha256Digest,
    plan_id: Sha256Digest,
    status: CredentialReceiptStatus,
    recovery_digest: Sha256Digest,
    destinations: Vec<CredentialRecoveryDestination>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct LegacyCredentialReceipt {
    schema: String,
    receipt_id: Sha256Digest,
    status: String,
}

#[derive(Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
enum CredentialReceiptStatus {
    Applying,
    Succeeded,
    RolledBack,
    RecoveryRequired,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CredentialRecoveryDestination {
    destination_id: StableId,
    path: NormalizedManagedPath,
    backup: DurableCredentialBackup,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    verification_tag: Option<Sha256Digest>,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
enum DurableCredentialBackup {
    File { digest: Sha256Digest },
    Absent,
    Unchanged,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CredentialPlanRequest {
    destination_ids: Vec<StableId>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
struct CredentialApplyRequest {
    plan_id: Sha256Digest,
    confirmed: Option<bool>,
    confirmation_id: Option<StableId>,
    idempotency_key: Option<String>,
}
impl CredentialApplyRequest {
    fn is_confirmed(&self) -> bool {
        self.confirmed == Some(true)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CredentialVerifyRequest {
    destination_ids: Vec<StableId>,
}

fn credential_observed_digest(
    root: &Path,
    path: &NormalizedManagedPath,
) -> Result<Sha256Digest, DomainFailure> {
    let path = root.join(path.as_str());
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            digest_domain_json(
                "commonkit.credentials.observed.file.v1",
                &digest_bytes(&fs::read(path).map_err(|_| DomainFailure::OperationFailed)?)?,
            )
            .map_err(|_| DomainFailure::OperationFailed)
        }
        Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {
            digest_text("commonkit.credentials.observed.kind.v1", "directory")
        }
        Ok(metadata) if metadata.file_type().is_symlink() => {
            digest_text("commonkit.credentials.observed.kind.v1", "symlink")
        }
        Ok(_) => digest_text("commonkit.credentials.observed.kind.v1", "other"),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            digest_text("commonkit.credentials.observed.kind.v1", "absent")
        }
        Err(_) => Err(DomainFailure::OperationFailed),
    }
}

impl ProductionCredentialDomain {
    fn state_root(&self) -> PathBuf {
        self.root.join(CREDENTIAL_STATE_DIRECTORY)
    }

    fn plan_path(&self, plan_id: &Sha256Digest) -> PathBuf {
        self.state_root().join("plans").join(format!(
            "{}.json",
            plan_id.as_str().trim_start_matches("sha256:")
        ))
    }

    fn receipt_path(&self, receipt_id: &Sha256Digest) -> PathBuf {
        self.state_root().join("receipts").join(format!(
            "{}.json",
            receipt_id.as_str().trim_start_matches("sha256:")
        ))
    }

    fn verification_key_path(&self) -> PathBuf {
        self.state_root().join("verification.key")
    }

    fn load_or_create_verification_key(&self) -> Result<[u8; 32], DomainFailure> {
        let path = self.verification_key_path();
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                let bytes = fs::read(path).map_err(|_| DomainFailure::OperationFailed)?;
                bytes.try_into().map_err(|_| DomainFailure::OperationFailed)
            }
            Ok(_) => Err(DomainFailure::OperationFailed),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                let mut key = [0_u8; 32];
                rand::rng().fill_bytes(&mut key);
                write_private_atomic_with_sync(&path, &key, self.directory_sync.as_ref())?;
                Ok(key)
            }
            Err(_) => Err(DomainFailure::OperationFailed),
        }
    }

    fn load_verification_key(&self) -> Result<[u8; 32], DomainFailure> {
        let path = self.verification_key_path();
        let metadata =
            fs::symlink_metadata(&path).map_err(|_| DomainFailure::VerificationFailed)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(DomainFailure::VerificationFailed);
        }
        fs::read(path)
            .map_err(|_| DomainFailure::VerificationFailed)?
            .try_into()
            .map_err(|_| DomainFailure::VerificationFailed)
    }

    fn verification_tag(
        key: &[u8; 32],
        destination: &StableId,
        path: &NormalizedManagedPath,
        reference_digest: &Sha256Digest,
        bytes: &[u8],
    ) -> Result<Sha256Digest, DomainFailure> {
        let mut message = Vec::new();
        message.extend_from_slice(destination.as_str().as_bytes());
        message.push(0);
        message.extend_from_slice(path.as_str().as_bytes());
        message.push(0);
        message.extend_from_slice(reference_digest.as_str().as_bytes());
        message.push(0);
        message.extend_from_slice(bytes);
        Sha256Digest::parse(format!("sha256:{}", hmac_sha256(key, &message)))
            .map_err(|_| DomainFailure::OperationFailed)
    }

    fn backup_path(&self, receipt_id: &Sha256Digest, digest: &Sha256Digest) -> PathBuf {
        self.state_root()
            .join("backups")
            .join(receipt_id.as_str().trim_start_matches("sha256:"))
            .join(digest.as_str().trim_start_matches("sha256:"))
    }

    fn write_receipt(&self, receipt: &CredentialReceipt) -> Result<(), DomainFailure> {
        let bytes = serde_json::to_vec(receipt).map_err(|_| DomainFailure::OperationFailed)?;
        let path = self.receipt_path(&receipt.receipt_id);
        write_private_atomic_with_sync(&path, &bytes, self.directory_sync.as_ref())
    }

    fn cleanup_backups(&self, receipt_id: &Sha256Digest) -> Result<(), DomainFailure> {
        let backup_root = self.state_root().join("backups");
        let run_root = backup_root.join(receipt_id.as_str().trim_start_matches("sha256:"));
        match fs::remove_dir_all(run_root) {
            Ok(()) => sync_parent_directory_with(&backup_root, self.directory_sync.as_ref()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(DomainFailure::OperationFailed),
        }
    }

    fn cleanup_orphan_backups(&self, referenced: &BTreeSet<String>) -> Result<(), DomainFailure> {
        let root = self.state_root().join("backups");
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(_) => return Err(DomainFailure::OperationFailed),
        };
        for entry in entries {
            let entry = entry.map_err(|_| DomainFailure::OperationFailed)?;
            let metadata = entry
                .file_type()
                .map_err(|_| DomainFailure::OperationFailed)?;
            if metadata.is_symlink() || !metadata.is_dir() {
                return Err(DomainFailure::OperationFailed);
            }
            let name = entry
                .file_name()
                .into_string()
                .map_err(|_| DomainFailure::OperationFailed)?;
            if !referenced.contains(&name) {
                fs::remove_dir_all(entry.path()).map_err(|_| DomainFailure::OperationFailed)?;
            }
        }
        sync_parent_directory_with(&root, self.directory_sync.as_ref())
    }

    fn build_plan(&self, ids: Vec<StableId>) -> Result<StoredCredentialPlan, DomainFailure> {
        if ids.is_empty() {
            return Err(DomainFailure::InvalidRequest);
        }
        let mut unique = BTreeSet::new();
        let mut destinations = Vec::new();
        for id in ids {
            if !unique.insert(id.clone()) {
                return Err(DomainFailure::InvalidRequest);
            }
            let destination = self
                .destinations
                .get(&id)
                .ok_or(DomainFailure::InvalidRequest)?;
            destinations.push(PlannedCredentialDestination {
                destination_id: id,
                path: destination.path.clone(),
                reference_scheme: destination.reference.scheme().to_owned(),
                reference_digest: digest_text(
                    "commonkit.credentials.reference.v1",
                    destination.reference.as_str(),
                )?,
                observed_digest: credential_observed_digest(&self.root, &destination.path)?,
            });
        }
        destinations.sort_by(|left, right| left.destination_id.cmp(&right.destination_id));
        let plan_id = digest_domain_json("commonkit.credentials.plan.v1", &destinations)
            .map_err(|_| DomainFailure::OperationFailed)?;
        Ok(StoredCredentialPlan {
            schema: "commonkit.credentials.plan.v1".into(),
            plan_id,
            destinations,
        })
    }

    fn persist_plan(&self, plan: &StoredCredentialPlan) -> Result<(), DomainFailure> {
        let bytes = serde_json::to_vec(plan).map_err(|_| DomainFailure::OperationFailed)?;
        write_private_atomic_with_sync(
            &self.plan_path(&plan.plan_id),
            &bytes,
            self.directory_sync.as_ref(),
        )
    }

    fn load_plan(&self, plan_id: &Sha256Digest) -> Result<StoredCredentialPlan, DomainFailure> {
        let bytes = fs::read(self.plan_path(plan_id)).map_err(|_| DomainFailure::StalePlan)?;
        let plan: StoredCredentialPlan =
            serde_json::from_slice(&bytes).map_err(|_| DomainFailure::StalePlan)?;
        if &plan.plan_id != plan_id
            || digest_domain_json("commonkit.credentials.plan.v1", &plan.destinations)
                .map_err(|_| DomainFailure::OperationFailed)?
                != *plan_id
        {
            return Err(DomainFailure::StalePlan);
        }
        Ok(plan)
    }

    fn capture_backup(
        &self,
        receipt_id: &Sha256Digest,
        path: &NormalizedManagedPath,
    ) -> Result<DurableCredentialBackup, DomainFailure> {
        let path = self.root.join(path.as_str());
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                let bytes = fs::read(path).map_err(|_| DomainFailure::OperationFailed)?;
                let digest = digest_bytes(&bytes).map_err(|_| DomainFailure::OperationFailed)?;
                let backup_path = self.backup_path(receipt_id, &digest);
                if backup_path.exists() {
                    let existing =
                        fs::read(&backup_path).map_err(|_| DomainFailure::OperationFailed)?;
                    if digest_bytes(&existing).map_err(|_| DomainFailure::OperationFailed)?
                        != digest
                    {
                        return Err(DomainFailure::OperationFailed);
                    }
                } else {
                    write_private_atomic_with_sync(
                        &backup_path,
                        &bytes,
                        self.directory_sync.as_ref(),
                    )?;
                }
                Ok(DurableCredentialBackup::File { digest })
            }
            Ok(_) => Ok(DurableCredentialBackup::Unchanged),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                Ok(DurableCredentialBackup::Absent)
            }
            Err(_) => Err(DomainFailure::OperationFailed),
        }
    }

    fn load_backup(
        &self,
        receipt_id: &Sha256Digest,
        backup: &DurableCredentialBackup,
    ) -> Result<Option<Vec<u8>>, DomainFailure> {
        let DurableCredentialBackup::File { digest } = backup else {
            return Ok(None);
        };
        let bytes = fs::read(self.backup_path(receipt_id, digest))
            .map_err(|_| DomainFailure::OperationFailed)?;
        if digest_bytes(&bytes).map_err(|_| DomainFailure::OperationFailed)? != *digest {
            return Err(DomainFailure::OperationFailed);
        }
        Ok(Some(bytes))
    }

    fn restore_backup(
        &self,
        store: &LocalSensitiveFileStore,
        receipt_id: &Sha256Digest,
        path: &NormalizedManagedPath,
        backup: &DurableCredentialBackup,
    ) -> Result<(), DomainFailure> {
        match backup {
            DurableCredentialBackup::File { .. } => store
                .restore_backup_bytes(
                    path,
                    &self
                        .load_backup(receipt_id, backup)?
                        .ok_or(DomainFailure::OperationFailed)?,
                )
                .map_err(|_| DomainFailure::OperationFailed),
            DurableCredentialBackup::Absent => {
                let destination = self.root.join(path.as_str());
                remove_file_with_sync(&destination, self.directory_sync.as_ref())
            }
            DurableCredentialBackup::Unchanged => Ok(()),
        }
    }

    fn recover_unfinished(&self) -> Result<(), DomainFailure> {
        let receipts = self.state_root().join("receipts");
        let mut paths = match fs::read_dir(receipts) {
            Ok(entries) => entries
                .collect::<Result<Vec<_>, _>>()
                .map_err(|_| DomainFailure::OperationFailed)?
                .into_iter()
                .map(|entry| entry.path())
                .collect::<Vec<_>>(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(_) => return Err(DomainFailure::OperationFailed),
        };
        paths.sort();
        let mut referenced_backups = BTreeSet::new();
        let store = LocalSensitiveFileStore::open(&self.root)
            .map_err(|_| DomainFailure::OperationFailed)?;
        for path in paths {
            let metadata =
                fs::symlink_metadata(&path).map_err(|_| DomainFailure::OperationFailed)?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(DomainFailure::OperationFailed);
            }
            let bytes = fs::read(&path).map_err(|_| DomainFailure::OperationFailed)?;
            let value: Value =
                serde_json::from_slice(&bytes).map_err(|_| DomainFailure::OperationFailed)?;
            if value.get("schema").and_then(Value::as_str)
                == Some("commonkit.credentials.receipt.v1")
            {
                let legacy: LegacyCredentialReceipt =
                    serde_json::from_value(value).map_err(|_| DomainFailure::OperationFailed)?;
                if legacy.schema != "commonkit.credentials.receipt.v1"
                    || !matches!(legacy.status.as_str(), "succeeded" | "rolledBack")
                {
                    return Err(DomainFailure::OperationFailed);
                }
                referenced_backups.insert(
                    legacy
                        .receipt_id
                        .as_str()
                        .trim_start_matches("sha256:")
                        .to_owned(),
                );
                self.cleanup_backups(&legacy.receipt_id)?;
                continue;
            }
            let mut receipt: CredentialReceipt =
                serde_json::from_value(value).map_err(|_| DomainFailure::OperationFailed)?;
            referenced_backups.insert(
                receipt
                    .receipt_id
                    .as_str()
                    .trim_start_matches("sha256:")
                    .to_owned(),
            );
            if receipt.schema != "commonkit.credentials.receipt.v2"
                || path.file_stem().and_then(|name| name.to_str())
                    != Some(receipt.receipt_id.as_str().trim_start_matches("sha256:"))
                || digest_domain_json("commonkit.credentials.recovery.v1", &receipt.destinations)
                    .map_err(|_| DomainFailure::OperationFailed)?
                    != receipt.recovery_digest
            {
                return Err(DomainFailure::OperationFailed);
            }
            if !matches!(
                receipt.status,
                CredentialReceiptStatus::Applying | CredentialReceiptStatus::RecoveryRequired
            ) {
                self.cleanup_backups(&receipt.receipt_id)?;
                continue;
            }
            let plan = self
                .load_plan(&receipt.plan_id)
                .map_err(|_| DomainFailure::OperationFailed)?;
            if plan.destinations.len() != receipt.destinations.len()
                || plan
                    .destinations
                    .iter()
                    .zip(&receipt.destinations)
                    .any(|(planned, recovery)| {
                        planned.destination_id != recovery.destination_id
                            || planned.path != recovery.path
                    })
            {
                return Err(DomainFailure::OperationFailed);
            }
            for destination in &receipt.destinations {
                let configured = self
                    .destinations
                    .get(&destination.destination_id)
                    .ok_or(DomainFailure::OperationFailed)?;
                if configured.path != destination.path {
                    return Err(DomainFailure::OperationFailed);
                }
                let _ = self.load_backup(&receipt.receipt_id, &destination.backup)?;
            }
            for destination in receipt.destinations.iter().rev() {
                if self
                    .restore_backup(
                        &store,
                        &receipt.receipt_id,
                        &destination.path,
                        &destination.backup,
                    )
                    .is_err()
                {
                    receipt.status = CredentialReceiptStatus::RecoveryRequired;
                    let _ = self.write_receipt(&receipt);
                    return Err(DomainFailure::OperationFailed);
                }
            }
            receipt.status = CredentialReceiptStatus::RolledBack;
            self.write_receipt(&receipt)?;
            self.cleanup_backups(&receipt.receipt_id)?;
        }
        self.cleanup_orphan_backups(&referenced_backups)?;
        Ok(())
    }
}

fn ensure_private_parent_with_sync(
    path: &Path,
    directory_sync: &CredentialDirectorySync,
) -> Result<(), DomainFailure> {
    let parent = path.parent().ok_or(DomainFailure::OperationFailed)?;
    let mut missing = Vec::new();
    let mut cursor = parent;
    loop {
        match fs::symlink_metadata(cursor) {
            Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => break,
            Ok(_) => return Err(DomainFailure::OperationFailed),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                missing.push(cursor.to_path_buf());
                cursor = cursor.parent().ok_or(DomainFailure::OperationFailed)?;
            }
            Err(_) => return Err(DomainFailure::OperationFailed),
        }
    }
    for directory in missing.iter().rev() {
        fs::create_dir(directory).map_err(|_| DomainFailure::OperationFailed)?;
        #[cfg(unix)]
        fs::set_permissions(
            directory,
            std::os::unix::fs::PermissionsExt::from_mode(0o700),
        )
        .map_err(|_| DomainFailure::OperationFailed)?;
        directory_sync(directory).map_err(|_| DomainFailure::OperationFailed)?;
        let containing = directory.parent().ok_or(DomainFailure::OperationFailed)?;
        directory_sync(containing).map_err(|_| DomainFailure::OperationFailed)?;
    }
    Ok(())
}
fn resolve(
    reference: &CredentialReference,
    bws_executable: Option<&Path>,
) -> Result<SecretValue, DomainFailure> {
    let bytes = match reference.scheme() {
        "env" => std::env::var_os(reference.opaque())
            .map(|v| v.to_string_lossy().into_owned().into_bytes())
            .ok_or(DomainFailure::OperationFailed)?,
        "file" => {
            let metadata = fs::symlink_metadata(reference.opaque())
                .map_err(|_| DomainFailure::OperationFailed)?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(DomainFailure::OperationFailed);
            }
            fs::read(reference.opaque()).map_err(|_| DomainFailure::OperationFailed)?
        }
        "bws" => {
            let executable = bws_executable.ok_or(DomainFailure::OperationFailed)?;
            return BwsCredentialResolver::new(ProcessBwsRunner::new(executable))
                .resolve(reference)
                .map_err(|_| DomainFailure::OperationFailed);
        }
        "keychain" => {
            let platform =
                PlatformKeychain::current().map_err(|_| DomainFailure::OperationFailed)?;
            return PlatformKeychainCredentialResolver::new(
                platform,
                ProcessPlatformSecretCommandRunner,
            )
            .resolve(reference)
            .map_err(|_| DomainFailure::OperationFailed);
        }
        _ => return Err(DomainFailure::OperationFailed),
    };
    SecretValue::new(bytes).map_err(|_| DomainFailure::OperationFailed)
}

struct ProductionAboutMeDomain {
    config: AboutMeConfig,
}

impl ProductionAboutMeDomain {
    fn view(&self) -> ScopedView {
        ScopedView {
            loadout_id: self.config.loadout_id.clone(),
            project_id: self.config.project_id.clone(),
        }
    }

    fn store(&self) -> Result<ProfileStore, DomainFailure> {
        let secret = resolve(
            &self.config.key_reference,
            self.config.bws_executable.as_deref(),
        )?;
        ProfileStore::open(&self.config.database, secret.expose_for_apply())
            .map_err(|_| DomainFailure::OperationFailed)
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AboutMeSearchRequest {
    query: String,
    #[serde(default)]
    categories: BTreeSet<String>,
    limit: u8,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AboutMeSuggestionRequest {
    topic_key: String,
    category: ClaimCategory,
    text: String,
    evidence_quote: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AboutMeResolveRequest {
    active_claim_id: String,
    expected_revision: u64,
    replacement_text: String,
    evidence_quote: String,
    confirmed: bool,
    confirmation_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AboutMeDraftRequest {
    expected_revision: u64,
    summary: String,
    claims: Vec<AboutMeClaimRequest>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AboutMeClaimRequest {
    topic_key: String,
    category: ClaimCategory,
    text: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AboutMePublishRequest {
    draft_id: String,
    expected_revision: u64,
    confirmed: bool,
    confirmation_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct AboutMeDecisionRequest {
    suggestion_id: String,
    decision: commonkit_about_me::SuggestionDecision,
    confirmed: bool,
    confirmation_id: Option<String>,
}

impl AboutMeDomain for ProductionAboutMeDomain {
    fn inspect(&self) -> Result<Value, DomainFailure> {
        let store = self.store()?;
        let suggestions = store
            .pending_suggestions()
            .map_err(|_| DomainFailure::OperationFailed)?;
        Ok(serde_json::json!({
            "revision": store.revision().map_err(|_| DomainFailure::OperationFailed)?,
            "summary": store.summary(&self.view()).map_err(|_| DomainFailure::OperationFailed)?,
            "pendingSuggestions": suggestions.len(),
            "suggestions": suggestions,
            "loadoutId": self.config.loadout_id,
            "projectId": self.config.project_id,
            "encrypted": true
        }))
    }

    fn create_draft(&self, request: Value) -> Result<Value, DomainFailure> {
        let request: AboutMeDraftRequest =
            serde_json::from_value(request).map_err(|_| DomainFailure::InvalidRequest)?;
        let view = self.view();
        let claims = request
            .claims
            .into_iter()
            .map(|claim| ClaimInput {
                topic_key: claim.topic_key,
                category: claim.category,
                text: claim.text,
                views: vec![view.clone()],
            })
            .collect();
        let mut store = self.store()?;
        let draft = store
            .create_draft(request.expected_revision, request.summary, claims)
            .map_err(|error| match error {
                commonkit_about_me::ProfileError::StaleRevision => DomainFailure::StalePlan,
                _ => DomainFailure::InvalidRequest,
            })?;
        serde_json::to_value(draft).map_err(|_| DomainFailure::OperationFailed)
    }

    fn publish_draft(&self, request: Value) -> Result<Value, DomainFailure> {
        let request: AboutMePublishRequest =
            serde_json::from_value(request).map_err(|_| DomainFailure::InvalidRequest)?;
        if !request.confirmed {
            return Err(DomainFailure::InvalidRequest);
        }
        let _ = request.confirmation_id;
        let mut store = self.store()?;
        let published = store
            .publish_draft(&request.draft_id, request.expected_revision)
            .map_err(|error| match error {
                commonkit_about_me::ProfileError::StaleRevision => DomainFailure::StalePlan,
                _ => DomainFailure::OperationFailed,
            })?;
        serde_json::to_value(published).map_err(|_| DomainFailure::OperationFailed)
    }

    fn suggestions(&self) -> Result<Value, DomainFailure> {
        let store = self.store()?;
        let suggestions = store
            .pending_suggestions()
            .map_err(|_| DomainFailure::OperationFailed)?;
        Ok(serde_json::json!({"suggestions": suggestions}))
    }

    fn decide_suggestion(&self, request: Value) -> Result<Value, DomainFailure> {
        let request: AboutMeDecisionRequest =
            serde_json::from_value(request).map_err(|_| DomainFailure::InvalidRequest)?;
        if !request.confirmed {
            return Err(DomainFailure::InvalidRequest);
        }
        let _ = request.confirmation_id;
        let mut store = self.store()?;
        let published = store
            .decide_suggestion(&request.suggestion_id, request.decision)
            .map_err(|_| DomainFailure::OperationFailed)?;
        Ok(serde_json::json!({"published": published}))
    }

    fn summary(&self) -> Result<Value, DomainFailure> {
        let store = self.store()?;
        Ok(serde_json::json!({
            "revision": store.revision().map_err(|_| DomainFailure::OperationFailed)?,
            "summary": store.summary(&self.view()).map_err(|_| DomainFailure::OperationFailed)?,
        }))
    }

    fn search(&self, request: Value) -> Result<Value, DomainFailure> {
        let request: AboutMeSearchRequest =
            serde_json::from_value(request).map_err(|_| DomainFailure::InvalidRequest)?;
        let mut store = self.store()?;
        let mut results = store
            .search(&self.view(), &request.query, request.limit as usize)
            .map_err(|_| DomainFailure::OperationFailed)?;
        if !request.categories.is_empty() {
            results.retain(|result| request.categories.contains(result.category.as_str()));
        }
        serde_json::to_value(serde_json::json!({"claims": results}))
            .map_err(|_| DomainFailure::OperationFailed)
    }

    fn suggest(&self, request: Value) -> Result<Value, DomainFailure> {
        let request: AboutMeSuggestionRequest =
            serde_json::from_value(request).map_err(|_| DomainFailure::InvalidRequest)?;
        let view = self.view();
        let mut store = self.store()?;
        let suggestion = store
            .suggest(
                view.clone(),
                ClaimInput {
                    topic_key: request.topic_key,
                    category: request.category,
                    text: request.text,
                    views: vec![view],
                },
                &request.evidence_quote,
                &self.config.agent_id,
            )
            .map_err(|_| DomainFailure::OperationFailed)?;
        serde_json::to_value(suggestion).map_err(|_| DomainFailure::OperationFailed)
    }

    fn resolve_conflict(&self, request: Value) -> Result<Value, DomainFailure> {
        let request: AboutMeResolveRequest =
            serde_json::from_value(request).map_err(|_| DomainFailure::InvalidRequest)?;
        if !request.confirmed {
            return Err(DomainFailure::InvalidRequest);
        }
        let _ = request.confirmation_id;
        let mut store = self.store()?;
        let published = store
            .resolve_contradiction(
                &request.active_claim_id,
                request.expected_revision,
                &request.replacement_text,
                &request.evidence_quote,
            )
            .map_err(|error| match error {
                commonkit_about_me::ProfileError::StaleRevision => DomainFailure::StalePlan,
                _ => DomainFailure::OperationFailed,
            })?;
        serde_json::to_value(published).map_err(|_| DomainFailure::OperationFailed)
    }
}

impl CredentialDomain for ProductionCredentialDomain {
    fn plan(&self, request: Value) -> Result<Value, DomainFailure> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| DomainFailure::OperationFailed)?;
        let request: CredentialPlanRequest =
            serde_json::from_value(request).map_err(|_| DomainFailure::InvalidRequest)?;
        let plan = self.build_plan(request.destination_ids)?;
        self.persist_plan(&plan)?;
        let operations = plan
            .destinations
            .iter()
            .map(|destination| {
                serde_json::json!({
                    "destinationId": destination.destination_id,
                    "path": destination.path,
                    "action": "provision"
                })
            })
            .collect::<Vec<_>>();
        Ok(serde_json::json!({"planId": plan.plan_id, "operations": operations}))
    }
    fn apply(&self, request: Value) -> Result<Value, DomainFailure> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| DomainFailure::OperationFailed)?;
        let request: CredentialApplyRequest =
            serde_json::from_value(request).map_err(|_| DomainFailure::InvalidRequest)?;
        if !request.is_confirmed() {
            return Err(DomainFailure::InvalidRequest);
        }
        self.recover_unfinished()?;
        let plan_id = request.plan_id;
        let confirmation_id = request
            .confirmation_id
            .clone()
            .ok_or(DomainFailure::InvalidRequest)?;
        let plan = self.load_plan(&plan_id)?;
        for planned in &plan.destinations {
            let destination = self
                .destinations
                .get(&planned.destination_id)
                .ok_or(DomainFailure::StalePlan)?;
            let current = self.build_plan(vec![planned.destination_id.clone()])?;
            let current = current
                .destinations
                .first()
                .ok_or(DomainFailure::StalePlan)?;
            if current.path != planned.path
                || current.reference_digest != planned.reference_digest
                || current.observed_digest != planned.observed_digest
            {
                return Err(DomainFailure::StalePlan);
            }
            let _ = destination;
        }
        let store = LocalSensitiveFileStore::open(&self.root)
            .map_err(|_| DomainFailure::OperationFailed)?;
        let receipt_id = digest_domain_json(
            "commonkit.credentials.receipt.v1",
            &(
                plan_id.clone(),
                confirmation_id,
                request.idempotency_key.clone(),
            ),
        )
        .map_err(|_| DomainFailure::OperationFailed)?;
        let mut resolved = Vec::new();
        for planned in &plan.destinations {
            let destination = self
                .destinations
                .get(&planned.destination_id)
                .ok_or(DomainFailure::StalePlan)?;
            let secret = resolve(&destination.reference, self.bws_executable.as_deref())?;
            resolved.push((planned, destination, secret));
        }
        let recovery = resolved
            .iter()
            .map(|(planned, _, _)| {
                Ok(CredentialRecoveryDestination {
                    destination_id: planned.destination_id.clone(),
                    path: planned.path.clone(),
                    backup: self.capture_backup(&receipt_id, &planned.path)?,
                    verification_tag: None,
                })
            })
            .collect::<Result<Vec<_>, DomainFailure>>();
        let recovery = match recovery {
            Ok(recovery) => recovery,
            Err(error) => {
                let _ = self.cleanup_backups(&receipt_id);
                return Err(error);
            }
        };
        let recovery_digest = digest_domain_json("commonkit.credentials.recovery.v1", &recovery)
            .map_err(|_| DomainFailure::OperationFailed)?;
        let mut receipt = CredentialReceipt {
            schema: "commonkit.credentials.receipt.v2".into(),
            receipt_id: receipt_id.clone(),
            plan_id,
            status: CredentialReceiptStatus::Applying,
            recovery_digest,
            destinations: recovery,
        };
        // The applying checkpoint is durable only after every backup artifact exists.
        self.write_receipt(&receipt)?;
        for destination in &receipt.destinations {
            let _ = self.load_backup(&receipt_id, &destination.backup)?;
        }
        let mut applied = Vec::new();
        let verification_key = self.load_or_create_verification_key()?;
        for (index, (planned, destination, secret)) in resolved.iter().enumerate() {
            if store.write(&destination.path, secret).is_err() {
                let mut rollback_succeeded = true;
                // The failing write may have truncated or created its destination before
                // reporting an I/O or durability error, so restore it as well as all
                // previously completed writes.
                for rollback_index in (0..=index).rev() {
                    if self
                        .restore_backup(
                            &store,
                            &receipt_id,
                            &resolved[rollback_index].0.path,
                            &receipt.destinations[rollback_index].backup,
                        )
                        .is_err()
                    {
                        rollback_succeeded = false;
                    }
                }
                receipt.status = if rollback_succeeded {
                    CredentialReceiptStatus::RolledBack
                } else {
                    CredentialReceiptStatus::RecoveryRequired
                };
                self.write_receipt(&receipt)?;
                if receipt.status == CredentialReceiptStatus::RolledBack {
                    self.cleanup_backups(&receipt_id)?;
                }
                return Err(DomainFailure::OperationFailed);
            }
            receipt.destinations[index].verification_tag = Some(Self::verification_tag(
                &verification_key,
                &planned.destination_id,
                &planned.path,
                &planned.reference_digest,
                secret.expose_for_apply(),
            )?);
            receipt.recovery_digest =
                digest_domain_json("commonkit.credentials.recovery.v1", &receipt.destinations)
                    .map_err(|_| DomainFailure::OperationFailed)?;
            if self.write_receipt(&receipt).is_err() {
                let mut rollback_succeeded = true;
                for rollback_index in (0..=index).rev() {
                    if self
                        .restore_backup(
                            &store,
                            &receipt_id,
                            &resolved[rollback_index].0.path,
                            &receipt.destinations[rollback_index].backup,
                        )
                        .is_err()
                    {
                        rollback_succeeded = false;
                    }
                }
                receipt.status = if rollback_succeeded {
                    CredentialReceiptStatus::RolledBack
                } else {
                    CredentialReceiptStatus::RecoveryRequired
                };
                let _ = self.write_receipt(&receipt);
                return Err(DomainFailure::OperationFailed);
            }
            applied.push(planned.destination_id.clone());
        }
        receipt.status = CredentialReceiptStatus::Succeeded;
        if self.write_receipt(&receipt).is_err() {
            let mut rollback_succeeded = true;
            for rollback_index in (0..resolved.len()).rev() {
                if self
                    .restore_backup(
                        &store,
                        &receipt_id,
                        &resolved[rollback_index].0.path,
                        &receipt.destinations[rollback_index].backup,
                    )
                    .is_err()
                {
                    rollback_succeeded = false;
                }
            }
            receipt.status = if rollback_succeeded {
                CredentialReceiptStatus::RolledBack
            } else {
                CredentialReceiptStatus::RecoveryRequired
            };
            let _ = self.write_receipt(&receipt);
            return Err(DomainFailure::OperationFailed);
        }
        self.cleanup_backups(&receipt_id)?;
        Ok(serde_json::json!({"applied":applied,"receiptId":receipt_id,"planId":receipt.plan_id}))
    }
    fn verify(&self, request: Value) -> Result<Value, DomainFailure> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| DomainFailure::OperationFailed)?;
        let request: CredentialVerifyRequest =
            serde_json::from_value(request).map_err(|_| DomainFailure::InvalidRequest)?;
        let verification_key = self.load_verification_key()?;
        let receipt_root = self.state_root().join("receipts");
        let receipt_paths = fs::read_dir(receipt_root)
            .map_err(|_| DomainFailure::VerificationFailed)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(|_| DomainFailure::VerificationFailed)?;
        let mut verified = Vec::new();
        for id in request.destination_ids {
            let destination = self
                .destinations
                .get(&id)
                .ok_or(DomainFailure::InvalidRequest)?;
            let path = self.root.join(destination.path.as_str());
            let metadata =
                fs::symlink_metadata(&path).map_err(|_| DomainFailure::VerificationFailed)?;
            if !metadata.is_file() || metadata.file_type().is_symlink() {
                return Err(DomainFailure::VerificationFailed);
            }
            let bytes = fs::read(path).map_err(|_| DomainFailure::VerificationFailed)?;
            let reference_digest = digest_text(
                "commonkit.credentials.reference.v1",
                destination.reference.as_str(),
            )
            .map_err(|_| DomainFailure::VerificationFailed)?;
            let observed = Self::verification_tag(
                &verification_key,
                &id,
                &destination.path,
                &reference_digest,
                &bytes,
            )
            .map_err(|_| DomainFailure::VerificationFailed)?;
            let mut matched = false;
            for entry in &receipt_paths {
                let receipt: CredentialReceipt = serde_json::from_slice(
                    &fs::read(entry.path()).map_err(|_| DomainFailure::VerificationFailed)?,
                )
                .map_err(|_| DomainFailure::VerificationFailed)?;
                if receipt.status == CredentialReceiptStatus::Succeeded
                    && digest_domain_json(
                        "commonkit.credentials.recovery.v1",
                        &receipt.destinations,
                    )
                    .map_err(|_| DomainFailure::VerificationFailed)?
                        == receipt.recovery_digest
                    && receipt.destinations.iter().any(|record| {
                        record.destination_id == id
                            && record.path == destination.path
                            && record.verification_tag.as_ref() == Some(&observed)
                    })
                {
                    matched = true;
                    break;
                }
            }
            if !matched {
                return Err(DomainFailure::VerificationFailed);
            }
            verified.push(id);
        }
        Ok(serde_json::json!({"verified":verified}))
    }
}

struct ProductionSnapshotDomain {
    root: PathBuf,
    portable_state: PathBuf,
    key_reference: CredentialReference,
    object_store: SnapshotObjectStoreConfig,
    git_authority: Option<SnapshotGitAuthorityConfig>,
    databases: BTreeMap<String, SnapshotDatabase>,
    lock: Mutex<()>,
}
struct LocalObjects(PathBuf);
impl ObjectStore for LocalObjects {
    fn put(&mut self, key: &str, value: &[u8]) -> Result<(), SnapshotError> {
        fs::write(self.0.join(key.trim_start_matches("sha256:")), value)
            .map_err(|_| SnapshotError::ObjectStoreFailed)
    }
    fn get(&self, key: &str) -> Result<Vec<u8>, SnapshotError> {
        fs::read(self.0.join(key.trim_start_matches("sha256:")))
            .map_err(|_| SnapshotError::ObjectNotFound(key.into()))
    }
}
enum SnapshotObjects {
    Local(LocalObjects),
    S3(S3CompatibleObjectStore<ProcessObjectCommandRunner>),
}
impl ObjectStore for SnapshotObjects {
    fn put(&mut self, key: &str, value: &[u8]) -> Result<(), SnapshotError> {
        match self {
            Self::Local(store) => store.put(key, value),
            Self::S3(store) => store.put(key, value),
        }
    }
    fn get(&self, key: &str) -> Result<Vec<u8>, SnapshotError> {
        match self {
            Self::Local(store) => store.get(key),
            Self::S3(store) => store.get(key),
        }
    }
}
#[derive(Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredSnapshot {
    snapshot_id: StableId,
    manifest: SnapshotManifest,
}
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PortableSnapshotDescriptor {
    schema: String,
    snapshot_id: StableId,
    database_id: DatabaseId,
    encrypted_manifest_digest: Sha256Digest,
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
struct SnapshotRequest {
    database_id: Option<String>,
    snapshot_id: Option<StableId>,
    target_id: Option<String>,
    confirmed: Option<bool>,
    confirmation_id: Option<StableId>,
    idempotency_key: Option<String>,
}
impl SnapshotRequest {
    fn acknowledge_metadata(&self) {
        let _ = (self.confirmed, &self.confirmation_id, &self.idempotency_key);
    }
}
impl ProductionSnapshotDomain {
    fn portable_authority<'a>(
        &self,
        cipher: &'a XChaCha20Cipher,
    ) -> Result<PortableAuthorityStore<'a, XChaCha20Cipher>, SnapshotError> {
        PortableAuthorityStore::open_with_trusted_anchor(
            &self.portable_state,
            self.root.join("portable-authority-anchors"),
            cipher,
        )
    }

    fn read_authority_or_deferred(
        &self,
        database: &SnapshotDatabase,
        cipher: &XChaCha20Cipher,
    ) -> Result<Option<commonkit_snapshots::VersionedPortableAuthority>, DomainFailure> {
        let authority_path = self
            .portable_state
            .join("authority")
            .join(format!("{}.json", database.id));
        if authority_path.exists() {
            return self
                .portable_authority(cipher)
                .map_err(|_| DomainFailure::OperationFailed)?
                .read(&database.id)
                .map(Some)
                .map_err(|_| DomainFailure::VerificationFailed);
        }
        if self
            .portable_state
            .join("authority-history")
            .join(database.id.to_string())
            .exists()
        {
            return Err(DomainFailure::VerificationFailed);
        }
        for entry in fs::read_dir(self.portable_state.join("snapshots"))
            .map_err(|_| DomainFailure::OperationFailed)?
        {
            let path = entry.map_err(|_| DomainFailure::OperationFailed)?.path();
            let bytes = fs::read(&path).map_err(|_| DomainFailure::OperationFailed)?;
            let expected = path
                .file_stem()
                .and_then(|value| value.to_str())
                .ok_or(DomainFailure::VerificationFailed)?;
            if path.extension().and_then(|value| value.to_str()) != Some("json")
                || format!("{:x}", Sha256::digest(&bytes)) != expected
            {
                return Err(DomainFailure::VerificationFailed);
            }
            let descriptor: PortableSnapshotDescriptor =
                serde_json::from_slice(&bytes).map_err(|_| DomainFailure::VerificationFailed)?;
            if descriptor.schema != "commonkit.portable-snapshot-descriptor.v1" {
                return Err(DomainFailure::VerificationFailed);
            }
            if descriptor.database_id == database.id {
                return Err(DomainFailure::VerificationFailed);
            }
        }
        Ok(None)
    }

    fn initialize_authority(
        &self,
        database: &SnapshotDatabase,
        cipher: &XChaCha20Cipher,
    ) -> Result<commonkit_snapshots::VersionedPortableAuthority, DomainFailure> {
        let authority_preexisted = self
            .portable_state
            .join("authority")
            .join(format!("{}.json", database.id))
            .exists();
        let portable = self
            .portable_authority(cipher)
            .map_err(|_| DomainFailure::OperationFailed)?;
        let authority = portable
            .initialize(&database.id, &database.target_id)
            .map_err(|_| DomainFailure::VerificationFailed)?;
        if self.git_authority.is_some() {
            let mut publisher = self.git_authority_publisher()?;
            if !authority_preexisted
                && self
                    .git_authority
                    .as_ref()
                    .is_some_and(|config| config.bootstrap)
            {
                publisher
                    .bootstrap_remote_trust_anchor(
                        &self.portable_state,
                        &commonkit_snapshots::PortablePublication {
                            database: database.id.clone(),
                            generation: authority.record.generation,
                            authority_revision: authority.revision.clone(),
                        },
                    )
                    .map_err(|_| DomainFailure::VerificationFailed)?;
            }
            publisher
                .verify_remote_trust_anchor(
                    &database.id,
                    authority.record.generation,
                    &authority.revision,
                )
                .map_err(|_| DomainFailure::VerificationFailed)?;
        }
        AuthorityStore::open(self.root.join("authority"))
            .and_then(|local| {
                local.synchronize_from_portable(&database.id, &authority.record.current_writer)
            })
            .map_err(|_| DomainFailure::OperationFailed)?;
        Ok(authority)
    }

    fn git_authority_publisher(&self) -> Result<ProcessGitAuthorityPublisher, DomainFailure> {
        let config = self
            .git_authority
            .as_ref()
            .ok_or(DomainFailure::VerificationFailed)?;
        let portable_relative = self
            .portable_state
            .strip_prefix(&config.repository)
            .map_err(|_| DomainFailure::OperationFailed)?;
        ProcessGitAuthorityPublisher::new(
            &config.executable,
            &config.repository,
            &config.trusted_remote_url,
            &config.branch,
            portable_relative,
            &config.staging_root,
        )
        .map_err(|_| DomainFailure::OperationFailed)
    }
    fn database_path_for_target<'a>(
        database: &'a SnapshotDatabase,
        target: &str,
    ) -> Result<&'a Path, DomainFailure> {
        if target == database.target_id {
            Ok(&database.path)
        } else {
            database
                .observed_paths
                .get(target)
                .map(PathBuf::as_path)
                .ok_or(DomainFailure::InvalidRequest)
        }
    }

    fn observed_database_digest(
        database: &SnapshotDatabase,
        target: &str,
    ) -> Result<String, DomainFailure> {
        let path = Self::database_path_for_target(database, target)?;
        let bytes = match database.format {
            SnapshotSourceFormat::Sqlite => SqliteBackup::new(path)
                .export()
                .map_err(|_| DomainFailure::VerificationFailed)?,
            SnapshotSourceFormat::File => {
                fs::read(path).map_err(|_| DomainFailure::VerificationFailed)?
            }
        };
        Ok(format!("sha256:{:x}", Sha256::digest(bytes)))
    }

    /// Finds the sole authenticated history head by following content-digest parent links. The
    /// encrypted descriptors are content-addressed, but their snapshot IDs are deliberately not a
    /// clock and therefore must never be used to infer chronology.
    fn validated_chain_head(
        manifests: Vec<StoredSnapshot>,
        database_id: &str,
    ) -> Result<Option<StoredSnapshot>, DomainFailure> {
        let mut nodes = BTreeMap::<String, StoredSnapshot>::new();
        for stored in manifests
            .into_iter()
            .filter(|stored| stored.manifest.database.to_string() == database_id)
        {
            if stored.manifest.schema != "commonkit.snapshot.v1"
                || stored.manifest.source_digest != stored.manifest.content_digest
                || nodes
                    .insert(stored.manifest.content_digest.clone(), stored)
                    .is_some()
            {
                return Err(DomainFailure::VerificationFailed);
            }
        }
        if nodes.is_empty() {
            return Ok(None);
        }

        let mut parents_with_children = BTreeSet::new();
        let mut roots = 0usize;
        for (digest, stored) in &nodes {
            match &stored.manifest.parent_digest {
                None => roots += 1,
                Some(parent) => {
                    if parent == digest
                        || !nodes.contains_key(parent)
                        || !parents_with_children.insert(parent.clone())
                    {
                        return Err(DomainFailure::VerificationFailed);
                    }
                }
            }
        }
        let heads = nodes
            .keys()
            .filter(|digest| !parents_with_children.contains(*digest))
            .cloned()
            .collect::<Vec<_>>();
        if roots != 1 || heads.len() != 1 {
            return Err(DomainFailure::VerificationFailed);
        }

        let mut visited = BTreeSet::new();
        let mut cursor = heads[0].clone();
        loop {
            if !visited.insert(cursor.clone()) {
                return Err(DomainFailure::VerificationFailed);
            }
            match nodes
                .get(&cursor)
                .ok_or(DomainFailure::VerificationFailed)?
                .manifest
                .parent_digest
                .clone()
            {
                Some(parent) => cursor = parent,
                None => break,
            }
        }
        if visited.len() != nodes.len() {
            return Err(DomainFailure::VerificationFailed);
        }
        Ok(nodes.remove(&heads[0]))
    }

    fn recover_unfinished(&self) -> Result<(), ProductionDomainError> {
        let cipher = self
            .cipher()
            .map_err(|_| ProductionDomainError::UnsafeConfig)?;
        let transaction_root = self.root.join("transactions");
        let mut restores = DurableRestore::open(&transaction_root, &cipher)
            .map_err(|_| ProductionDomainError::UnsafeConfig)?;
        for plan in restores
            .unfinished_runs()
            .map_err(|_| ProductionDomainError::UnsafeConfig)?
        {
            let database = self
                .databases
                .get(&plan.database.to_string())
                .ok_or(ProductionDomainError::UnsafeConfig)?;
            let mut lifecycle = ProcessDatabaseLifecycle(&database.lifecycle);
            restores
                .recover(&plan.run_id, &database.path, &mut lifecycle)
                .map_err(|_| ProductionDomainError::UnsafeConfig)?;
        }
        let authority = AuthorityStore::open(self.root.join("authority"))
            .map_err(|_| ProductionDomainError::UnsafeConfig)?;
        let unfinished_promotions = authority
            .unfinished_promotions()
            .map_err(|_| ProductionDomainError::UnsafeConfig)?;
        if unfinished_promotions.is_empty() {
            return Ok(());
        }
        let portable = self
            .portable_authority(&cipher)
            .map_err(|_| ProductionDomainError::UnsafeConfig)?;
        for run_id in unfinished_promotions {
            let plan = authority
                .prepared_promotion(&run_id)
                .map_err(|_| ProductionDomainError::UnsafeConfig)?;
            let shared = portable
                .read(&plan.database)
                .map_err(|_| ProductionDomainError::UnsafeConfig)?;
            authority
                .recover_promotion_against_portable(
                    &run_id,
                    &shared.record.current_writer,
                    &shared.revision,
                )
                .map_err(|_| ProductionDomainError::UnsafeConfig)?;
        }
        Ok(())
    }

    fn objects(&self) -> Result<SnapshotObjects, DomainFailure> {
        match &self.object_store {
            SnapshotObjectStoreConfig::Local => Ok(SnapshotObjects::Local(LocalObjects(
                self.root.join("objects"),
            ))),
            SnapshotObjectStoreConfig::S3 {
                executable,
                endpoint,
                bucket,
                prefix,
            } => {
                if !executable.is_absolute() {
                    return Err(DomainFailure::OperationFailed);
                }
                S3CompatibleObjectStore::new(
                    ProcessObjectCommandRunner::new(executable),
                    endpoint.clone(),
                    bucket.clone(),
                    prefix.clone(),
                )
                .map(SnapshotObjects::S3)
                .map_err(|_| DomainFailure::OperationFailed)
            }
        }
    }
    fn cipher(&self) -> Result<XChaCha20Cipher, DomainFailure> {
        let secret = resolve(&self.key_reference, None)?;
        let mut key = [0u8; 32];
        key.copy_from_slice(&Sha256::digest(secret.expose_for_apply()));
        Ok(XChaCha20Cipher::new(key))
    }
    fn manifests(&self) -> Result<Vec<StoredSnapshot>, DomainFailure> {
        let mut output = BTreeMap::<StableId, StoredSnapshot>::new();
        let cipher = self.cipher()?;
        // Portable descriptors are the sole discovery/commit point. A failed create may leave an
        // immutable object-store orphan, but it can never expose an uncommitted local snapshot.
        for entry in fs::read_dir(self.portable_state.join("snapshots"))
            .map_err(|_| DomainFailure::OperationFailed)?
        {
            let path = entry.map_err(|_| DomainFailure::OperationFailed)?.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                return Err(DomainFailure::VerificationFailed);
            }
            let descriptor_bytes = fs::read(&path).map_err(|_| DomainFailure::OperationFailed)?;
            let expected = path
                .file_stem()
                .and_then(|value| value.to_str())
                .ok_or(DomainFailure::VerificationFailed)?;
            if format!("{:x}", Sha256::digest(&descriptor_bytes)) != expected {
                return Err(DomainFailure::VerificationFailed);
            }
            let descriptor: PortableSnapshotDescriptor = serde_json::from_slice(&descriptor_bytes)
                .map_err(|_| DomainFailure::VerificationFailed)?;
            if descriptor.schema != "commonkit.portable-snapshot-descriptor.v1" {
                return Err(DomainFailure::VerificationFailed);
            }
            let encrypted = self
                .objects()?
                .get(descriptor.encrypted_manifest_digest.as_str())
                .map_err(|_| DomainFailure::VerificationFailed)?;
            if format!("sha256:{:x}", Sha256::digest(&encrypted))
                != descriptor.encrypted_manifest_digest.as_str()
            {
                return Err(DomainFailure::VerificationFailed);
            }
            let plaintext = cipher
                .open(&encrypted, b"commonkit.snapshot-manifest.v1")
                .map_err(|_| DomainFailure::VerificationFailed)?;
            let stored: StoredSnapshot = serde_json::from_slice(&plaintext)
                .map_err(|_| DomainFailure::VerificationFailed)?;
            if stored.snapshot_id != descriptor.snapshot_id
                || stored.manifest.database != descriptor.database_id
            {
                return Err(DomainFailure::VerificationFailed);
            }
            output.insert(stored.snapshot_id.clone(), stored);
        }
        let all_manifests = output.into_values().collect::<Vec<_>>();
        let mut manifests = Vec::new();
        for database in self.databases.values() {
            let mut by_content = all_manifests
                .iter()
                .filter(|stored| stored.manifest.database == database.id)
                .map(|stored| (stored.manifest.content_digest.clone(), (*stored).clone()))
                .collect::<BTreeMap<_, _>>();
            if by_content.len()
                != all_manifests
                    .iter()
                    .filter(|stored| stored.manifest.database == database.id)
                    .count()
            {
                return Err(DomainFailure::VerificationFailed);
            }
            let Some(authority) = self.read_authority_or_deferred(database, &cipher)? else {
                if by_content.is_empty() {
                    continue;
                }
                return Err(DomainFailure::VerificationFailed);
            };
            let Some(mut cursor) = authority.record.accepted_head else {
                // Unaccepted descriptors are interrupted/concurrent object-store or Git orphans,
                // never history. They become visible only through an authority CAS.
                continue;
            };
            let mut accepted = Vec::new();
            let mut visited = BTreeSet::new();
            loop {
                if !visited.insert(cursor.clone()) {
                    return Err(DomainFailure::VerificationFailed);
                }
                let stored = by_content
                    .remove(&cursor)
                    .ok_or(DomainFailure::VerificationFailed)?;
                let parent = stored.manifest.parent_digest.clone();
                accepted.push(stored);
                match parent {
                    Some(parent) => cursor = parent,
                    None => break,
                }
            }
            accepted.reverse();
            manifests.extend(accepted);
        }
        if all_manifests.iter().any(|stored| {
            !self
                .databases
                .contains_key(&stored.manifest.database.to_string())
        }) {
            return Err(DomainFailure::VerificationFailed);
        }
        Ok(manifests)
    }

    fn write_portable_descriptor(
        &self,
        stored: &StoredSnapshot,
        encrypted_manifest: &[u8],
    ) -> Result<String, DomainFailure> {
        let encrypted_manifest_digest =
            Sha256Digest::parse(format!("sha256:{:x}", Sha256::digest(encrypted_manifest)))
                .map_err(|_| DomainFailure::OperationFailed)?;
        let descriptor = PortableSnapshotDescriptor {
            schema: "commonkit.portable-snapshot-descriptor.v1".into(),
            snapshot_id: stored.snapshot_id.clone(),
            database_id: stored.manifest.database.clone(),
            encrypted_manifest_digest: encrypted_manifest_digest.clone(),
        };
        let bytes = serde_json::to_vec(&descriptor).map_err(|_| DomainFailure::OperationFailed)?;
        let descriptor_digest = format!("{:x}", Sha256::digest(&bytes));
        let directory = self.portable_state.join("snapshots");
        let destination = directory.join(format!("{descriptor_digest}.json"));
        // The manifest is ciphertext before it crosses the portable-state boundary. Uploading and
        // verifying it first makes the descriptor rename the final discoverability commit point.
        if self
            .objects()?
            .get(encrypted_manifest_digest.as_str())
            .is_err()
        {
            return Err(DomainFailure::VerificationFailed);
        }
        if destination.exists() {
            return (fs::read(destination).map_err(|_| DomainFailure::OperationFailed)? == bytes)
                .then_some(format!("sha256:{descriptor_digest}"))
                .ok_or(DomainFailure::VerificationFailed);
        }
        let (mut temporary, temporary_path) = (0..16)
            .find_map(|_| {
                let path = directory.join(format!(".snapshot-{:032x}.tmp", rand::random::<u128>()));
                OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(&path)
                    .ok()
                    .map(|file| (file, path))
            })
            .ok_or(DomainFailure::OperationFailed)?;
        if temporary
            .write_all(&bytes)
            .and_then(|_| temporary.sync_all())
            .is_err()
        {
            let _ = fs::remove_file(&temporary_path);
            return Err(DomainFailure::OperationFailed);
        }
        drop(temporary);
        match fs::rename(&temporary_path, &destination) {
            Ok(()) => Ok(format!("sha256:{descriptor_digest}")),
            Err(_) if destination.exists() => {
                let _ = fs::remove_file(&temporary_path);
                (fs::read(destination).map_err(|_| DomainFailure::OperationFailed)? == bytes)
                    .then_some(format!("sha256:{descriptor_digest}"))
                    .ok_or(DomainFailure::VerificationFailed)
            }
            Err(_) => {
                let _ = fs::remove_file(&temporary_path);
                Err(DomainFailure::OperationFailed)
            }
        }
    }
    fn writers(&self) -> Result<BTreeMap<String, String>, DomainFailure> {
        let cipher = self.cipher()?;
        let mut writers = BTreeMap::new();
        for (id, database) in &self.databases {
            let writer = self
                .read_authority_or_deferred(database, &cipher)?
                .map(|authority| authority.record.current_writer)
                .unwrap_or_else(|| database.target_id.clone());
            writers.insert(id.clone(), writer);
        }
        Ok(writers)
    }
}

#[cfg(test)]
mod credential_durability_tests {
    use super::*;

    #[test]
    fn private_atomic_write_durably_creates_every_receipt_directory() {
        let temporary = tempfile::tempdir().unwrap();
        let root = temporary.path().join("credentials");
        fs::create_dir(&root).unwrap();
        let path = root
            .join(CREDENTIAL_STATE_DIRECTORY)
            .join("receipts")
            .join("receipt.json");
        let synced = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&synced);
        let directory_sync = move |path: &Path| {
            observed.lock().unwrap().push(path.to_path_buf());
            Ok(())
        };

        write_private_atomic_with_sync(&path, b"receipt", &directory_sync).unwrap();

        assert_eq!(fs::read(&path).unwrap(), b"receipt");
        assert_eq!(
            *synced.lock().unwrap(),
            vec![
                root.join(CREDENTIAL_STATE_DIRECTORY),
                root.clone(),
                root.join(CREDENTIAL_STATE_DIRECTORY).join("receipts"),
                root.join(CREDENTIAL_STATE_DIRECTORY),
                root.join(CREDENTIAL_STATE_DIRECTORY).join("receipts"),
            ]
        );
    }

    #[test]
    fn private_atomic_write_fails_closed_when_receipt_parent_sync_fails() {
        let temporary = tempfile::tempdir().unwrap();
        let parent = temporary.path().join("receipts");
        fs::create_dir(&parent).unwrap();
        let path = parent.join("receipt.json");
        let failing_sync = |_: &Path| Err(std::io::Error::other("injected sync failure"));

        assert_eq!(
            write_private_atomic_with_sync(&path, b"receipt", &failing_sync),
            Err(DomainFailure::OperationFailed)
        );
    }

    #[test]
    fn credential_delete_fails_closed_when_destination_parent_sync_fails() {
        let temporary = tempfile::tempdir().unwrap();
        let parent = temporary.path().join("credentials");
        fs::create_dir(&parent).unwrap();
        let path = parent.join("token");
        fs::write(&path, b"secret").unwrap();
        let failing_sync = |_: &Path| Err(std::io::Error::other("injected sync failure"));

        assert_eq!(
            remove_file_with_sync(&path, &failing_sync),
            Err(DomainFailure::OperationFailed)
        );
        assert!(!path.exists());
    }
}

#[cfg(test)]
mod snapshot_chain_tests {
    use super::*;

    fn stored(id: &str, content: &str, parent: Option<&str>) -> StoredSnapshot {
        StoredSnapshot {
            snapshot_id: StableId::parse(id).unwrap(),
            manifest: SnapshotManifest {
                schema: "commonkit.snapshot.v1".into(),
                database: DatabaseId::new("context-mode").unwrap(),
                source_target: "writer".into(),
                source_format: "file".into(),
                source_digest: content.into(),
                parent_digest: parent.map(str::to_owned),
                content_digest: content.into(),
                object_digest: format!("object-{id}"),
                cipher: "test".into(),
            },
        }
    }

    #[test]
    fn snapshot_head_follows_parent_chain_not_lexicographic_snapshot_id() {
        let older = stored("snapshot-z", "sha256:older", None);
        let newer = stored("snapshot-a", "sha256:newer", Some("sha256:older"));

        let head =
            ProductionSnapshotDomain::validated_chain_head(vec![newer, older], "context-mode")
                .unwrap()
                .unwrap();

        assert_eq!(head.snapshot_id.as_str(), "snapshot-a");
    }

    #[test]
    fn snapshot_head_rejects_forks_cycles_and_missing_parents() {
        let fork = vec![
            stored("snapshot-root", "sha256:root", None),
            stored("snapshot-left", "sha256:left", Some("sha256:root")),
            stored("snapshot-right", "sha256:right", Some("sha256:root")),
        ];
        assert!(matches!(
            ProductionSnapshotDomain::validated_chain_head(fork, "context-mode"),
            Err(DomainFailure::VerificationFailed)
        ));

        let cycle = vec![
            stored("snapshot-one", "sha256:one", Some("sha256:two")),
            stored("snapshot-two", "sha256:two", Some("sha256:one")),
        ];
        assert!(matches!(
            ProductionSnapshotDomain::validated_chain_head(cycle, "context-mode"),
            Err(DomainFailure::VerificationFailed)
        ));

        let missing = vec![stored(
            "snapshot-orphan",
            "sha256:orphan",
            Some("sha256:missing"),
        )];
        assert!(matches!(
            ProductionSnapshotDomain::validated_chain_head(missing, "context-mode"),
            Err(DomainFailure::VerificationFailed)
        ));
    }
}

struct ProcessDatabaseLifecycle<'a>(&'a DatabaseLifecycleConfig);
impl ProcessDatabaseLifecycle<'_> {
    fn run(command: &LifecycleCommand) -> Result<(), SnapshotError> {
        if !command.executable.is_absolute() {
            return Err(SnapshotError::LifecycleFailed);
        }
        let metadata = fs::symlink_metadata(&command.executable)
            .map_err(|_| SnapshotError::LifecycleFailed)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(SnapshotError::LifecycleFailed);
        }
        let status = Command::new(&command.executable)
            .args(&command.args)
            .status()
            .map_err(|_| SnapshotError::LifecycleFailed)?;
        if status.success() {
            Ok(())
        } else {
            Err(SnapshotError::LifecycleFailed)
        }
    }
}
impl DatabaseLifecycle for ProcessDatabaseLifecycle<'_> {
    fn stop(&mut self) -> Result<(), SnapshotError> {
        Self::run(&self.0.stop)
    }
    fn start(&mut self) -> Result<(), SnapshotError> {
        Self::run(&self.0.start)
    }
}
impl SnapshotDomain for ProductionSnapshotDomain {
    fn create(&self, request: Value) -> Result<Value, DomainFailure> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| DomainFailure::OperationFailed)?;
        let request: SnapshotRequest =
            serde_json::from_value(request).map_err(|_| DomainFailure::InvalidRequest)?;
        request.acknowledge_metadata();
        let id = request.database_id.ok_or(DomainFailure::InvalidRequest)?;
        let database = self
            .databases
            .get(&id)
            .ok_or(DomainFailure::InvalidRequest)?;
        let cipher = self.cipher()?;
        let portable_authority = self.initialize_authority(database, &cipher)?;
        let source_target = portable_authority.record.current_writer.clone();
        let source_path = Self::database_path_for_target(database, &source_target)?;
        let service = SnapshotService::new(&cipher);
        let head = Self::validated_chain_head(self.manifests()?, &id)?;
        if portable_authority.record.accepted_head.as_deref()
            != head
                .as_ref()
                .map(|stored| stored.manifest.content_digest.as_str())
        {
            return Err(DomainFailure::VerificationFailed);
        }
        let prior = head
            .as_ref()
            .map(|item| item.manifest.content_digest.clone());
        let (source_format, bytes) = match database.format {
            SnapshotSourceFormat::Sqlite => (
                "sqlite3-online-backup",
                SqliteBackup::new(source_path)
                    .export()
                    .map_err(|_| DomainFailure::OperationFailed)?,
            ),
            SnapshotSourceFormat::File => (
                "file",
                fs::read(source_path).map_err(|_| DomainFailure::OperationFailed)?,
            ),
        };
        let source_digest = format!("sha256:{:x}", Sha256::digest(&bytes));
        if let Some(head) = head {
            if head.manifest.content_digest == source_digest {
                return Ok(serde_json::json!({"snapshotId":head.snapshot_id,"databaseId":id}));
            }
        }
        let mut objects = self.objects()?;
        let manifest = service
            .snapshot(
                &database.id,
                &source_target,
                prior,
                &StaticBackup::new(source_format, bytes),
                &mut objects,
            )
            .map_err(|_| DomainFailure::OperationFailed)?;
        let snapshot_id = StableId::parse(format!("snapshot-{}", &manifest.object_digest[7..31]))
            .map_err(|_| DomainFailure::OperationFailed)?;
        let stored = StoredSnapshot {
            snapshot_id: snapshot_id.clone(),
            manifest,
        };
        let manifest_bytes =
            serde_json::to_vec(&stored).map_err(|_| DomainFailure::OperationFailed)?;
        let encrypted_manifest = cipher
            .seal(&manifest_bytes, b"commonkit.snapshot-manifest.v1")
            .map_err(|_| DomainFailure::OperationFailed)?;
        let encrypted_digest = format!("{:x}", Sha256::digest(&encrypted_manifest));
        let encrypted_object_digest = format!("sha256:{encrypted_digest}");
        self.objects()?
            .put(&encrypted_object_digest, &encrypted_manifest)
            .map_err(|_| DomainFailure::OperationFailed)?;
        let descriptor_digest = self.write_portable_descriptor(&stored, &encrypted_manifest)?;
        let authority_store = self
            .portable_authority(&cipher)
            .map_err(|_| DomainFailure::OperationFailed)?;
        let authority_result = if self.git_authority.is_some() {
            let mut git_publisher = self.git_authority_publisher()?;
            let checked_out_revision = git_publisher
                .checked_out_revision()
                .map_err(|_| DomainFailure::VerificationFailed)?;
            let trusted_remote_revision = git_publisher
                .trusted_remote_revision()
                .map_err(|_| DomainFailure::VerificationFailed)?;
            if checked_out_revision != trusted_remote_revision {
                return Err(DomainFailure::VerificationFailed);
            }
            authority_store
                .compare_and_swap_head_published(
                    &database.id,
                    &portable_authority.revision,
                    PortableHeadUpdate {
                        writer: &source_target,
                        accepted_head: &stored.manifest.content_digest,
                        accepted_descriptor: &descriptor_digest,
                    },
                    &trusted_remote_revision,
                    &mut git_publisher,
                )
                .map(|published| published.authority)
        } else {
            authority_store.compare_and_swap_head(
                &database.id,
                &portable_authority.revision,
                &source_target,
                &stored.manifest.content_digest,
                &descriptor_digest,
            )
        };
        let updated_authority = match authority_result {
            Ok(authority) => authority,
            Err(_) => {
                // A descriptor is discoverable only when the authority CAS references it. Remove a
                // losing local candidate; never remove a descriptor accepted by a concurrent winner.
                let accepted = authority_store
                    .read(&database.id)
                    .ok()
                    .and_then(|authority| authority.record.accepted_descriptor);
                if accepted.as_deref() != Some(descriptor_digest.as_str()) {
                    let _ = fs::remove_file(
                        self.portable_state
                            .join("snapshots")
                            .join(format!("{}.json", &descriptor_digest[7..])),
                    );
                }
                return Err(DomainFailure::VerificationFailed);
            }
        };
        Ok(serde_json::json!({
            "snapshotId":snapshot_id,
            "databaseId":id,
            "authorityRevision":updated_authority.revision,
            "authorityGeneration":updated_authority.record.generation
        }))
    }
    fn list(&self) -> Result<Value, DomainFailure> {
        let writers = self.writers()?;
        let cipher = self.cipher()?;
        let authority_revisions = self
            .databases
            .iter()
            .map(|(id, database)| {
                let authority = self.read_authority_or_deferred(database, &cipher)?;
                Ok((
                    id.clone(),
                    authority.map_or_else(
                        || {
                            serde_json::json!({
                                "revision": null,
                                "generation": 0,
                                "acceptedHead": null,
                                "pendingInitialization": true
                            })
                        },
                        |authority| {
                            serde_json::json!({
                                "revision":authority.revision,
                                "generation":authority.record.generation,
                                "acceptedHead":authority.record.accepted_head,
                                "pendingInitialization": false
                            })
                        },
                    ),
                ))
            })
            .collect::<Result<BTreeMap<_, _>, DomainFailure>>()?;
        let promotion_evidence = self
            .databases
            .iter()
            .map(|(id, database)| {
                let mut observable_targets = vec![database.target_id.clone()];
                observable_targets.extend(database.observed_paths.keys().cloned());
                observable_targets.sort();
                (
                    id.clone(),
                    serde_json::json!({
                        "currentWriter": writers.get(id),
                        "observableTargets": observable_targets,
                        "requirement": "both current writer and candidate must be observable and match the latest snapshot"
                    }),
                )
            })
            .collect::<BTreeMap<_, _>>();
        Ok(serde_json::json!({
            "snapshots":self.manifests()?,
            "writers":writers,
            "authority":authority_revisions,
            "promotionEvidence":promotion_evidence
        }))
    }
    fn restore(&self, request: Value) -> Result<Value, DomainFailure> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| DomainFailure::OperationFailed)?;
        let request: SnapshotRequest =
            serde_json::from_value(request).map_err(|_| DomainFailure::InvalidRequest)?;
        request.acknowledge_metadata();
        let snapshot_id = request.snapshot_id.ok_or(DomainFailure::InvalidRequest)?;
        let stored = self
            .manifests()?
            .into_iter()
            .find(|item| item.snapshot_id == snapshot_id)
            .ok_or(DomainFailure::InvalidRequest)?;
        let database = self
            .databases
            .get(&stored.manifest.database.to_string())
            .ok_or(DomainFailure::InvalidRequest)?;
        let cipher = self.cipher()?;
        let run_id = request
            .confirmation_id
            .as_ref()
            .map(ToString::to_string)
            .unwrap_or_else(|| format!("restore-{snapshot_id}"));
        let plan = RestorePlan {
            schema: "commonkit.restore-plan.v1".into(),
            run_id,
            snapshot_id: snapshot_id.to_string(),
            manifest_digest: manifest_digest(&stored.manifest)
                .map_err(|_| DomainFailure::VerificationFailed)?,
            database: stored.manifest.database.clone(),
            expected_content_digest: stored.manifest.content_digest.clone(),
        };
        let receipt = DurableRestore::open(self.root.join("transactions"), &cipher)
            .map_err(|_| DomainFailure::OperationFailed)?
            .execute(
                plan,
                &stored.manifest,
                &self.objects()?,
                &database.path,
                &mut ProcessDatabaseLifecycle(&database.lifecycle),
                RestoreFailpoint::None,
            )
            .map_err(|_| DomainFailure::VerificationFailed)?;
        Ok(serde_json::json!({"snapshotId":snapshot_id,"restored":true,"receipt":receipt}))
    }
    fn promote(&self, request: Value) -> Result<Value, DomainFailure> {
        let _guard = self
            .lock
            .lock()
            .map_err(|_| DomainFailure::OperationFailed)?;
        let request: SnapshotRequest =
            serde_json::from_value(request).map_err(|_| DomainFailure::InvalidRequest)?;
        request.acknowledge_metadata();
        let id = request.database_id.ok_or(DomainFailure::InvalidRequest)?;
        let database = self
            .databases
            .get(&id)
            .ok_or(DomainFailure::InvalidRequest)?;
        if !self
            .manifests()?
            .iter()
            .any(|item| item.manifest.database.to_string() == id)
        {
            return Err(DomainFailure::InvalidRequest);
        }
        let target = request.target_id.ok_or(DomainFailure::InvalidRequest)?;
        let latest = Self::validated_chain_head(self.manifests()?, &id)?
            .ok_or(DomainFailure::InvalidRequest)?;
        let cipher = self.cipher()?;
        let portable_store = self
            .portable_authority(&cipher)
            .map_err(|_| DomainFailure::OperationFailed)?;
        let mut git_publisher = self.git_authority_publisher()?;
        let checked_out_revision = git_publisher
            .checked_out_revision()
            .map_err(|_| DomainFailure::VerificationFailed)?;
        let trusted_remote_revision = git_publisher
            .trusted_remote_revision()
            .map_err(|_| DomainFailure::VerificationFailed)?;
        let portable_authority = portable_store
            .read_at_repository_revision(
                &database.id,
                &checked_out_revision,
                &trusted_remote_revision,
            )
            .map_err(|_| DomainFailure::VerificationFailed)?;
        if portable_authority.record.accepted_head.as_deref()
            != Some(latest.manifest.content_digest.as_str())
        {
            return Err(DomainFailure::VerificationFailed);
        }
        let current_writer = portable_authority.record.current_writer.clone();
        let current_writer_digest = Self::observed_database_digest(database, &current_writer)?;
        let candidate_digest = Self::observed_database_digest(database, &target)?;
        let promotion = PromotionPlan {
            schema: "commonkit.promotion-plan.v1".into(),
            run_id: request
                .confirmation_id
                .map(|id| id.to_string())
                .unwrap_or_else(|| format!("promote-{}-{target}", id)),
            database: database_id(&id)?,
            previous_writer: current_writer.clone(),
            candidate_writer: target.clone(),
            latest_snapshot_digest: latest.manifest.content_digest.clone(),
            current_writer_digest,
            candidate_digest,
        };
        let local_authority = AuthorityStore::open(self.root.join("authority"))
            .map_err(|_| DomainFailure::OperationFailed)?;
        local_authority
            .prepare_promotion(promotion.clone())
            .map_err(|_| DomainFailure::VerificationFailed)?;
        let published_authority = portable_store
            .compare_and_swap_writer_published(
                &database.id,
                &portable_authority.revision,
                &current_writer,
                &target,
                &trusted_remote_revision,
                &mut git_publisher,
            )
            .map_err(|_| DomainFailure::VerificationFailed)?;
        let repository_revision = published_authority.repository_revision;
        let updated_authority = published_authority.authority;
        local_authority
            .confirm_portable_promotion(
                &promotion.run_id,
                &updated_authority.record.current_writer,
                &updated_authority.revision,
            )
            .map_err(|_| DomainFailure::VerificationFailed)?;
        local_authority
            .synchronize_from_portable(&database.id, &updated_authority.record.current_writer)
            .map_err(|_| DomainFailure::VerificationFailed)?;
        let receipt = local_authority
            .commit_prepared_promotion(&promotion.run_id)
            .map_err(|_| DomainFailure::VerificationFailed)?;
        Ok(serde_json::json!({
            "databaseId":id,
            "writer":target,
            "authorityRevision":updated_authority.revision,
            "authorityGeneration":updated_authority.record.generation,
            "repositoryRevision":repository_revision,
            "receipt":receipt
        }))
    }
}

fn database_id(value: &str) -> Result<DatabaseId, DomainFailure> {
    DatabaseId::new(value).map_err(|_| DomainFailure::InvalidRequest)
}

#[derive(Debug, Error)]
pub enum ProductionDomainError {
    #[error("production domain configuration is unsafe")]
    UnsafeConfig,
    #[error("configured capability is incomplete")]
    EmptyCapability,
    #[error("configured identifier is duplicated")]
    DuplicateId,
    #[error("configured composition cannot be materialized safely")]
    InvalidComposition,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
