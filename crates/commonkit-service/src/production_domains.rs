use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};

use commonkit_adapters::{
    ApmProvider, ApmProviderConfig, ArtifactStore, BwsCredentialResolver, ChezmoiProvider,
    ContentSensitivity, CredentialReference, CredentialResolver, DesiredStateProvider,
    ExactProviderVersion, FileAdapter, FilesystemIntent, GitRepository, GitSyncDisposition,
    LocalSensitiveFileStore, MaterializedState, NativeProvider, NormalizedManagedPath,
    NormalizedResource, OpenSshConfig, OpenSshTransport, OwnershipRules, PlatformKeychain,
    PlatformKeychainCredentialResolver, ProcessBwsRunner, ProcessGitRunner,
    ProcessPlatformSecretCommandRunner, ProcessRemoteRunner, ProviderCapability, ProviderContext,
    ProviderInputs, ProviderPipeline, ProviderPlanRequest, ProviderResourcePlanner,
    RemoteProviderStager, ResourceProvenance, SecretValue, SshFileAdapter, build_provider_plan,
    materialize_mcp_client_state, validate_ownership,
};
use commonkit_config::{LayerSet, MergeRules, compose_layers};
use commonkit_contracts::{
    LayerDocument, Plan, ReceiptState, Sha256Digest, StableId, assert_no_embedded_secrets,
    digest_domain_json,
};
use commonkit_reconcile::{
    Adapter, PlanStore, ReceiptError, ReceiptStore, ReconcileOutcome, Reconciler,
};
use commonkit_snapshots::{
    AuthenticatedCipher, Authority, AuthorityStore, ConsistentBackup, DatabaseId,
    DatabaseLifecycle, DurableRestore, ObjectStore, ProcessObjectCommandRunner, PromotionPlan,
    RestoreFailpoint, RestorePlan, S3CompatibleObjectStore, SnapshotError, SnapshotManifest,
    SnapshotService, SqliteBackup, StaticBackup, XChaCha20Cipher, manifest_digest,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::skill_canary::{SkillCanaryConfig, SkillCanaryRuntime};
use crate::{
    ApplyStatus, CompositionDomain, CredentialDomain, DomainFailure, ExecutionResult,
    HeadlessDomainRegistry, PlanExecutor, RelayProviderAuthority, SnapshotDomain, SyncDomain,
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProductionConfig {
    targets: Option<TargetInventoryConfig>,
    composition: Option<CompositionConfig>,
    sync: Option<SyncConfig>,
    #[serde(default)]
    sync_targets: Vec<SyncConfig>,
    credentials: Option<CredentialConfig>,
    snapshots: Option<SnapshotConfig>,
    skill_canary: Option<SkillCanaryConfig>,
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
}

#[derive(Clone, Deserialize)]
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

#[derive(Clone, Deserialize)]
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

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProviderPipelineConfig {
    root: PathBuf,
    source: GitProviderSource,
    providers: Vec<ConfiguredProvider>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct GitProviderSource {
    repository: PathBuf,
    trusted_remote_url: String,
    revision: String,
}

#[derive(Clone, Deserialize)]
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
        managed_root: NormalizedManagedPath,
    },
    Chezmoi {
        executable: PathBuf,
        source: PathBuf,
        config: PathBuf,
    },
}

#[derive(Clone, Deserialize)]
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
    databases: Vec<SnapshotDatabase>,
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
    pub composition: Option<Arc<dyn CompositionDomain>>,
    pub sync: Option<Arc<dyn SyncDomain>>,
    pub credentials: Option<Arc<dyn CredentialDomain>>,
    pub snapshots: Option<Arc<dyn SnapshotDomain>>,
    pub(crate) skill_canary: Option<Arc<SkillCanaryRuntime>>,
    pub targets: Option<Arc<crate::TargetInventory>>,
    pub target_sync_domains: BTreeMap<StableId, Arc<dyn SyncDomain>>,
    sync_configs: BTreeMap<StableId, SyncConfig>,
    ssh_execution: Option<(PathBuf, ProductionSshTarget)>,
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
                composition: None,
                sync: None,
                credentials: None,
                snapshots: None,
                skill_canary: None,
                targets: None,
                target_sync_domains: BTreeMap::new(),
                sync_configs: BTreeMap::new(),
                ssh_execution: None,
            });
        }
        Self::load(config_path, plan_store, receipt_root)
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
        let metadata = fs::symlink_metadata(config_path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(ProductionDomainError::UnsafeConfig);
        }
        let config: ProductionConfig = serde_json::from_slice(&fs::read(config_path)?)?;
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
        let mut target_sync_domains = BTreeMap::new();
        for (id, sync_config) in &sync_configs {
            validate_sync_config(sync_config)?;
            target_sync_domains.insert(
                id.clone(),
                Arc::new(ProductionSyncDomain {
                    config: sync_config.clone(),
                    plan_store: plan_store.clone(),
                    receipt_root: receipt_root.clone(),
                    ssh_factory: ssh_factory.clone(),
                }) as Arc<dyn SyncDomain>,
            );
        }
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
                    Ok(Arc::new(ProductionCredentialDomain {
                        root: config.root,
                        bws_executable: config.bws_executable,
                        destinations: unique_destinations(config.destinations)?,
                    }))
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
                    let authorities = AuthorityStore::open(config.root.join("authority"))
                        .map_err(|_| ProductionDomainError::UnsafeConfig)?;
                    for database in databases.values() {
                        authorities
                            .initialize(&database.id, &database.target_id)
                            .map_err(|_| ProductionDomainError::UnsafeConfig)?;
                    }
                    let domain = ProductionSnapshotDomain {
                        root: config.root,
                        portable_state: config.portable_state,
                        key_reference: config.key_reference,
                        object_store: config.object_store,
                        databases,
                        lock: Mutex::new(()),
                    };
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
            composition,
            sync,
            credentials,
            snapshots,
            skill_canary,
            targets,
            target_sync_domains,
            sync_configs,
            ssh_execution,
        })
    }
    pub fn into_headless(self) -> HeadlessDomainRegistry {
        HeadlessDomainRegistry {
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
    Ok(())
}

fn unique_destinations(
    values: Vec<CredentialDestination>,
) -> Result<BTreeMap<StableId, CredentialDestination>, ProductionDomainError> {
    let mut output = BTreeMap::new();
    for value in values {
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
            layers.push(
                serde_json::from_value::<LayerDocument>(value)
                    .map_err(|_| DomainFailure::OperationFailed)?,
            );
        }
        let layers = LayerSet::new(layers).map_err(|_| DomainFailure::OperationFailed)?;
        compose_layers(&layers, &MergeRules::new()).map_err(|_| DomainFailure::OperationFailed)
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
}

struct ProductionSyncDomain {
    config: SyncConfig,
    plan_store: Arc<PlanStore>,
    receipt_root: PathBuf,
    ssh_factory: Arc<dyn ProductionSshTransportFactory>,
}
impl ProductionSyncDomain {
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
        let status = repository
            .inspect(true)
            .map_err(|_| DomainFailure::OperationFailed)?;
        if status.revision.as_str() != config.source.revision
            || matches!(
                status.disposition,
                GitSyncDisposition::Dirty
                    | GitSyncDisposition::Behind
                    | GitSyncDisposition::Diverged
            )
        {
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
            .map_err(|_| DomainFailure::OperationFailed)
    }

    fn configured_provider(
        &self,
        config: &ConfiguredProvider,
        pipeline: &ProviderPipelineConfig,
        artifacts: &ArtifactStore,
    ) -> Result<Box<dyn DesiredStateProvider>, DomainFailure> {
        match config {
            ConfiguredProvider::Native { version, files } => {
                if files.is_empty() {
                    return Err(DomainFailure::OperationFailed);
                }
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
    fn build(&self) -> Result<Plan, DomainFailure> {
        fs::create_dir_all(&self.config.adapter_state)
            .map_err(|_| DomainFailure::OperationFailed)?;
        let artifacts = ArtifactStore::open(&self.config.provider_artifacts)
            .map_err(|_| DomainFailure::OperationFailed)?;
        let rules = OwnershipRules::new(
            self.config.case_sensitive,
            self.config.declared_roots.clone(),
            self.config.protected_roots.clone(),
        )
        .map_err(|_| DomainFailure::OperationFailed)?;
        let mut states = self.states()?;
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
                &artifacts,
                managed_root,
                "http://127.0.0.1:3764/mcp",
            )
            .map_err(|_| DomainFailure::OperationFailed)?
            {
                states.push(client_state);
            }
        }
        let resources = states
            .iter()
            .flat_map(|state| state.resources.iter().cloned())
            .collect::<Vec<_>>();
        validate_ownership(&resources, &rules).map_err(|_| DomainFailure::OperationFailed)?;
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
                        .map_err(|_| DomainFailure::OperationFailed)?;
                let observed = files
                    .observed_state_digest(intents())
                    .map_err(|_| DomainFailure::OperationFailed)?;
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
        self.plan_store
            .persist(&plan)
            .map_err(|_| DomainFailure::OperationFailed)?;
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
                ownership_rules: &rules,
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
        Ok(serde_json::json!({"planId":plan.id,"verified":true}))
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
}
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
struct CredentialRequest {
    destination_ids: Vec<StableId>,
    confirmed: Option<bool>,
    confirmation_id: Option<StableId>,
    idempotency_key: Option<String>,
}
impl CredentialRequest {
    fn acknowledge_metadata(&self) {
        let _ = (self.confirmed, &self.confirmation_id, &self.idempotency_key);
    }
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
impl CredentialDomain for ProductionCredentialDomain {
    fn apply(&self, request: Value) -> Result<Value, DomainFailure> {
        let request: CredentialRequest =
            serde_json::from_value(request).map_err(|_| DomainFailure::InvalidRequest)?;
        request.acknowledge_metadata();
        let store = LocalSensitiveFileStore::open(&self.root)
            .map_err(|_| DomainFailure::OperationFailed)?;
        let mut applied = Vec::new();
        for id in request.destination_ids {
            let destination = self
                .destinations
                .get(&id)
                .ok_or(DomainFailure::InvalidRequest)?;
            store
                .write(
                    &destination.path,
                    &resolve(&destination.reference, self.bws_executable.as_deref())?,
                )
                .map_err(|_| DomainFailure::OperationFailed)?;
            applied.push(id);
        }
        Ok(serde_json::json!({"applied":applied}))
    }
    fn verify(&self, request: Value) -> Result<Value, DomainFailure> {
        let request: CredentialRequest =
            serde_json::from_value(request).map_err(|_| DomainFailure::InvalidRequest)?;
        request.acknowledge_metadata();
        let mut verified = Vec::new();
        for id in request.destination_ids {
            let destination = self
                .destinations
                .get(&id)
                .ok_or(DomainFailure::InvalidRequest)?;
            let expected = resolve(&destination.reference, self.bws_executable.as_deref())?;
            let path = self.root.join(destination.path.as_str());
            let metadata =
                fs::symlink_metadata(&path).map_err(|_| DomainFailure::VerificationFailed)?;
            if !metadata.is_file()
                || metadata.file_type().is_symlink()
                || fs::read(path).map_err(|_| DomainFailure::VerificationFailed)?
                    != expected.expose_for_apply()
            {
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
#[derive(Serialize, Deserialize)]
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
    fn observed_database_digest(
        database: &SnapshotDatabase,
        target: &str,
    ) -> Result<String, DomainFailure> {
        let path = if target == database.target_id {
            &database.path
        } else {
            database
                .observed_paths
                .get(target)
                .ok_or(DomainFailure::InvalidRequest)?
        };
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
        for run_id in authority
            .unfinished_promotions()
            .map_err(|_| ProductionDomainError::UnsafeConfig)?
        {
            authority
                .recover_promotion(&run_id)
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
        Ok(output.into_values().collect())
    }

    fn write_portable_descriptor(
        &self,
        stored: &StoredSnapshot,
        encrypted_manifest: &[u8],
    ) -> Result<(), DomainFailure> {
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
                .then_some(())
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
            Ok(()) => Ok(()),
            Err(_) if destination.exists() => {
                let _ = fs::remove_file(&temporary_path);
                (fs::read(destination).map_err(|_| DomainFailure::OperationFailed)? == bytes)
                    .then_some(())
                    .ok_or(DomainFailure::VerificationFailed)
            }
            Err(_) => {
                let _ = fs::remove_file(&temporary_path);
                Err(DomainFailure::OperationFailed)
            }
        }
    }
    fn writers(&self) -> Result<BTreeMap<String, String>, DomainFailure> {
        let store = AuthorityStore::open(self.root.join("authority"))
            .map_err(|_| DomainFailure::OperationFailed)?;
        let mut writers = BTreeMap::new();
        for (id, database) in &self.databases {
            match store
                .authority(&database.id)
                .map_err(|_| DomainFailure::VerificationFailed)?
            {
                Authority::Writer { target } => {
                    writers.insert(id.clone(), target);
                }
                Authority::Unassigned => return Err(DomainFailure::VerificationFailed),
            }
        }
        Ok(writers)
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
        let service = SnapshotService::new(&cipher);
        let prior = self
            .manifests()?
            .into_iter()
            .filter(|item| item.manifest.database.to_string() == id)
            .last()
            .map(|item| item.manifest.content_digest);
        let mut objects = self.objects()?;
        let manifest = match database.format {
            SnapshotSourceFormat::Sqlite => service.snapshot(
                &database.id,
                &database.target_id,
                prior,
                &SqliteBackup::new(&database.path),
                &mut objects,
            ),
            SnapshotSourceFormat::File => {
                let bytes = fs::read(&database.path).map_err(|_| DomainFailure::OperationFailed)?;
                service.snapshot(
                    &database.id,
                    &database.target_id,
                    prior,
                    &StaticBackup::new("file", bytes),
                    &mut objects,
                )
            }
        }
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
        self.write_portable_descriptor(&stored, &encrypted_manifest)?;
        Ok(serde_json::json!({"snapshotId":snapshot_id,"databaseId":id}))
    }
    fn list(&self) -> Result<Value, DomainFailure> {
        let writers = self.writers()?;
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
        let latest = self
            .manifests()?
            .into_iter()
            .filter(|item| item.manifest.database.to_string() == id)
            .last()
            .ok_or(DomainFailure::InvalidRequest)?;
        let current_writer = self
            .writers()?
            .remove(&id)
            .ok_or(DomainFailure::VerificationFailed)?;
        let current_writer_digest = Self::observed_database_digest(database, &current_writer)?;
        let candidate_digest = Self::observed_database_digest(database, &target)?;
        let receipt = AuthorityStore::open(self.root.join("authority"))
            .map_err(|_| DomainFailure::OperationFailed)?
            .promote(PromotionPlan {
                schema: "commonkit.promotion-plan.v1".into(),
                run_id: request
                    .confirmation_id
                    .map(|id| id.to_string())
                    .unwrap_or_else(|| format!("promote-{}-{target}", id)),
                database: database_id(&id)?,
                previous_writer: current_writer,
                candidate_writer: target.clone(),
                latest_snapshot_digest: latest.manifest.content_digest.clone(),
                current_writer_digest,
                candidate_digest,
            })
            .map_err(|_| DomainFailure::VerificationFailed)?;
        Ok(serde_json::json!({"databaseId":id,"writer":target,"receipt":receipt}))
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
