use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use commonkit_adapters::{
    ArtifactStore, CredentialReference, FileAdapter, LocalSensitiveFileStore, MaterializedState,
    NormalizedManagedPath, OwnershipRules, ProviderPlanRequest, SecretValue, build_provider_plan,
    validate_ownership,
};
use commonkit_contracts::{Plan, Sha256Digest, StableId};
use commonkit_reconcile::{Adapter, PlanStore, ReceiptStore, Reconciler};
use commonkit_snapshots::{
    AuthenticatedCipher, DatabaseId, ObjectStore, ProcessObjectCommandRunner,
    S3CompatibleObjectStore, SnapshotError, SnapshotManifest, SnapshotService, SqliteBackup,
    StaticBackup, XChaCha20Cipher,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{CredentialDomain, DomainFailure, HeadlessDomainRegistry, SnapshotDomain, SyncDomain};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ProductionConfig {
    sync: Option<SyncConfig>,
    credentials: Option<CredentialConfig>,
    snapshots: Option<SnapshotConfig>,
}

#[derive(Clone, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SyncConfig {
    target_id: StableId,
    target_root: PathBuf,
    adapter_state: PathBuf,
    provider_artifacts: PathBuf,
    materialized_states: Vec<PathBuf>,
    declared_roots: Vec<NormalizedManagedPath>,
    protected_roots: Vec<NormalizedManagedPath>,
    case_sensitive: bool,
    target_identity_digest: Sha256Digest,
    composed_loadout_digest: Sha256Digest,
    policy_digest: Sha256Digest,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct CredentialConfig {
    root: PathBuf,
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
    format: SnapshotSourceFormat,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
enum SnapshotSourceFormat {
    Sqlite,
    File,
}

pub struct ProductionDomainRegistry {
    pub sync: Option<Arc<dyn SyncDomain>>,
    pub credentials: Option<Arc<dyn CredentialDomain>>,
    pub snapshots: Option<Arc<dyn SnapshotDomain>>,
}

impl ProductionDomainRegistry {
    pub fn load_optional(
        config_path: &Path,
        plan_store: Arc<PlanStore>,
        receipt_root: PathBuf,
    ) -> Result<Self, ProductionDomainError> {
        if !config_path.exists() {
            return Ok(Self {
                sync: None,
                credentials: None,
                snapshots: None,
            });
        }
        Self::load(config_path, plan_store, receipt_root)
    }
    pub fn load(
        config_path: &Path,
        plan_store: Arc<PlanStore>,
        receipt_root: PathBuf,
    ) -> Result<Self, ProductionDomainError> {
        let metadata = fs::symlink_metadata(config_path)?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(ProductionDomainError::UnsafeConfig);
        }
        let config: ProductionConfig = serde_json::from_slice(&fs::read(config_path)?)?;
        let sync = config
            .sync
            .map(
                |config| -> Result<Arc<dyn SyncDomain>, ProductionDomainError> {
                    if config.materialized_states.is_empty()
                        || config.declared_roots.is_empty()
                        || !config.target_root.is_absolute()
                        || !config.adapter_state.is_absolute()
                        || !config.provider_artifacts.is_absolute()
                    {
                        return Err(ProductionDomainError::EmptyCapability);
                    }
                    Ok(Arc::new(ProductionSyncDomain {
                        config,
                        plan_store: plan_store.clone(),
                        receipt_root: receipt_root.clone(),
                    }))
                },
            )
            .transpose()?;
        let credentials = config
            .credentials
            .map(
                |config| -> Result<Arc<dyn CredentialDomain>, ProductionDomainError> {
                    if config.destinations.is_empty() || !config.root.is_absolute() {
                        return Err(ProductionDomainError::EmptyCapability);
                    }
                    fs::create_dir_all(&config.root)?;
                    Ok(Arc::new(ProductionCredentialDomain {
                        root: config.root,
                        destinations: unique_destinations(config.destinations)?,
                    }))
                },
            )
            .transpose()?;
        let snapshots = config
            .snapshots
            .map(
                |config| -> Result<Arc<dyn SnapshotDomain>, ProductionDomainError> {
                    if config.databases.is_empty() || !config.root.is_absolute() {
                        return Err(ProductionDomainError::EmptyCapability);
                    }
                    validate_object_store(&config.object_store)?;
                    fs::create_dir_all(config.root.join("objects"))?;
                    fs::create_dir_all(config.root.join("manifests"))?;
                    Ok(Arc::new(ProductionSnapshotDomain {
                        root: config.root,
                        key_reference: config.key_reference,
                        object_store: config.object_store,
                        databases: unique_databases(config.databases)?,
                        lock: Mutex::new(()),
                    }))
                },
            )
            .transpose()?;
        Ok(Self {
            sync,
            credentials,
            snapshots,
        })
    }
    pub fn into_headless(self) -> HeadlessDomainRegistry {
        HeadlessDomainRegistry {
            sync: self.sync,
            credentials: self.credentials,
            snapshots: self.snapshots,
        }
    }
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

struct ProductionSyncDomain {
    config: SyncConfig,
    plan_store: Arc<PlanStore>,
    receipt_root: PathBuf,
}
impl ProductionSyncDomain {
    fn states(&self) -> Result<Vec<MaterializedState>, DomainFailure> {
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
    fn build(&self) -> Result<Plan, DomainFailure> {
        let artifacts = ArtifactStore::open(&self.config.provider_artifacts)
            .map_err(|_| DomainFailure::OperationFailed)?;
        let mut files = FileAdapter::open(&self.config.target_root, &self.config.adapter_state)
            .map_err(|_| DomainFailure::OperationFailed)?;
        let rules = OwnershipRules::new(
            self.config.case_sensitive,
            self.config.declared_roots.clone(),
            self.config.protected_roots.clone(),
        )
        .map_err(|_| DomainFailure::OperationFailed)?;
        let states = self.states()?;
        let resources = states
            .iter()
            .flat_map(|state| state.resources.iter().cloned())
            .collect::<Vec<_>>();
        validate_ownership(&resources, &rules).map_err(|_| DomainFailure::OperationFailed)?;
        let observed_digest = files
            .observed_state_digest(
                states
                    .iter()
                    .flat_map(|state| state.resources.iter().map(|resource| &resource.intent)),
            )
            .map_err(|_| DomainFailure::OperationFailed)?;
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
            &states,
            &artifacts,
            &mut files,
        )
        .map_err(|_| DomainFailure::OperationFailed)?;
        self.plan_store
            .persist(&plan)
            .map_err(|_| DomainFailure::OperationFailed)?;
        Ok(plan)
    }
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
        let mut adapter = FileAdapter::open(&self.config.target_root, &self.config.adapter_state)
            .map_err(|_| DomainFailure::OperationFailed)?;
        for operation in &plan.operations {
            adapter
                .verify(operation)
                .map_err(|_| DomainFailure::VerificationFailed)?;
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
        let mut adapters: Vec<Box<dyn Adapter>> = vec![Box::new(
            FileAdapter::open(&self.config.target_root, &self.config.adapter_state)
                .map_err(|_| DomainFailure::OperationFailed)?,
        )];
        let outcome = Reconciler::with_store(&receipts)
            .rollback_succeeded_run(run_id.clone(), &plan, &mut adapters)
            .map_err(|_| DomainFailure::OperationFailed)?;
        Ok(serde_json::json!({"runId":run_id,"outcome":format!("{outcome:?}").to_lowercase()}))
    }
}

struct ProductionCredentialDomain {
    root: PathBuf,
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
fn resolve(reference: &CredentialReference) -> Result<SecretValue, DomainFailure> {
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
                .write(&destination.path, &resolve(&destination.reference)?)
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
            let expected = resolve(&destination.reference)?;
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
        let secret = resolve(&self.key_reference)?;
        let mut key = [0u8; 32];
        key.copy_from_slice(&Sha256::digest(secret.expose_for_apply()));
        Ok(XChaCha20Cipher::new(key))
    }
    fn manifests(&self) -> Result<Vec<StoredSnapshot>, DomainFailure> {
        let mut output: Vec<StoredSnapshot> = Vec::new();
        let cipher = self.cipher()?;
        for entry in
            fs::read_dir(self.root.join("manifests")).map_err(|_| DomainFailure::OperationFailed)?
        {
            let path = entry.map_err(|_| DomainFailure::OperationFailed)?.path();
            if path.extension().and_then(|v| v.to_str()) != Some("bin") {
                return Err(DomainFailure::OperationFailed);
            }
            let snapshot_id = path
                .file_stem()
                .and_then(|value| value.to_str())
                .ok_or(DomainFailure::OperationFailed)?;
            let plaintext = cipher
                .open(
                    &fs::read(&path).map_err(|_| DomainFailure::OperationFailed)?,
                    format!("commonkit.snapshot-manifest.v1\0{snapshot_id}").as_bytes(),
                )
                .map_err(|_| DomainFailure::VerificationFailed)?;
            output.push(
                serde_json::from_slice(&plaintext).map_err(|_| DomainFailure::OperationFailed)?,
            );
        }
        output.sort_by(|a, b| a.snapshot_id.cmp(&b.snapshot_id));
        Ok(output)
    }
    fn writers(&self) -> Result<BTreeMap<String, String>, DomainFailure> {
        let path = self.root.join("writers.json");
        if !path.exists() {
            return Ok(self
                .databases
                .iter()
                .map(|(id, d)| (id.clone(), d.target_id.clone()))
                .collect());
        }
        serde_json::from_slice(&fs::read(path).map_err(|_| DomainFailure::OperationFailed)?)
            .map_err(|_| DomainFailure::OperationFailed)
    }

    fn install_restored_bytes(
        &self,
        database: &SnapshotDatabase,
        snapshot_id: &StableId,
        expected_digest: &str,
        bytes: &[u8],
    ) -> Result<(), DomainFailure> {
        let temporary = database
            .path
            .with_extension(format!("commonkit-{snapshot_id}.tmp"));
        let backup = database
            .path
            .with_extension(format!("commonkit-{snapshot_id}.previous"));
        fs::write(&temporary, bytes).map_err(|_| DomainFailure::OperationFailed)?;
        fs::rename(&database.path, &backup).map_err(|_| DomainFailure::OperationFailed)?;
        if fs::rename(&temporary, &database.path).is_err() {
            let _ = fs::rename(&backup, &database.path);
            return Err(DomainFailure::OperationFailed);
        }
        let current = fs::read(&database.path).map_err(|_| DomainFailure::OperationFailed)?;
        let current_digest = format!("sha256:{:x}", Sha256::digest(&current));
        if current_digest != expected_digest {
            let _ = fs::remove_file(&database.path);
            let _ = fs::rename(&backup, &database.path);
            return Err(DomainFailure::VerificationFailed);
        }
        fs::remove_file(backup).map_err(|_| DomainFailure::OperationFailed)
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
            .seal(
                &manifest_bytes,
                format!("commonkit.snapshot-manifest.v1\0{snapshot_id}").as_bytes(),
            )
            .map_err(|_| DomainFailure::OperationFailed)?;
        fs::write(
            self.root
                .join("manifests")
                .join(format!("{snapshot_id}.bin")),
            encrypted_manifest,
        )
        .map_err(|_| DomainFailure::OperationFailed)?;
        Ok(serde_json::json!({"snapshotId":snapshot_id,"databaseId":id}))
    }
    fn list(&self) -> Result<Value, DomainFailure> {
        Ok(serde_json::json!({"snapshots":self.manifests()?,"writers":self.writers()?}))
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
        let bytes = SnapshotService::new(&cipher)
            .verify_and_decrypt(&stored.manifest, &self.objects()?)
            .map_err(|_| DomainFailure::VerificationFailed)?;
        self.install_restored_bytes(
            database,
            &snapshot_id,
            &stored.manifest.content_digest,
            &bytes,
        )?;
        Ok(serde_json::json!({"snapshotId":snapshot_id,"restored":true}))
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
        if !self.databases.contains_key(&id)
            || !self
                .manifests()?
                .iter()
                .any(|item| item.manifest.database.to_string() == id)
        {
            return Err(DomainFailure::InvalidRequest);
        }
        let target = request.target_id.ok_or(DomainFailure::InvalidRequest)?;
        let mut writers = self.writers()?;
        writers.insert(id.clone(), target.clone());
        fs::write(
            self.root.join("writers.json"),
            serde_json::to_vec(&writers).map_err(|_| DomainFailure::OperationFailed)?,
        )
        .map_err(|_| DomainFailure::OperationFailed)?;
        Ok(serde_json::json!({"databaseId":id,"writer":target}))
    }
}

#[derive(Debug, Error)]
pub enum ProductionDomainError {
    #[error("production domain configuration is unsafe")]
    UnsafeConfig,
    #[error("configured capability is incomplete")]
    EmptyCapability,
    #[error("configured identifier is duplicated")]
    DuplicateId,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
}
