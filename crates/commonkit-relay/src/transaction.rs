use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use commonkit_contracts::{
    Operation, OperationKind, ResourceRef, Risk, Sha256Digest, StableId, digest_domain_json,
};
use commonkit_core::{OperationDraft, finalize_operation};
use commonkit_reconcile::{Adapter, AdapterFailure};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{RelayConfig, RelayConfigError};

static NONCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RelayMutationInputs {
    pub provider_inputs_digest: Sha256Digest,
    pub policy_digest: Sha256Digest,
    pub target_digest: Sha256Digest,
}

#[derive(Debug, Clone)]
pub struct RelayPlanRequest {
    pub desired: RelayConfig,
    pub inputs: RelayMutationInputs,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct RelayPayload {
    desired: RelayConfig,
    inputs: RelayMutationInputs,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OperationRecord {
    operation_id: Sha256Digest,
    payload: RelayPayload,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BackupRecord {
    operation_id: Sha256Digest,
    before_digest: Option<Sha256Digest>,
    bytes: Option<Vec<u8>>,
}

#[derive(Debug, Error)]
pub enum RelayPlanError {
    #[error("relay transaction state is unsafe or corrupt")]
    UnsafeState,
    #[error("relay transaction I/O failed")]
    Io(#[from] std::io::Error),
    #[error("relay transaction serialization failed")]
    Json(#[from] serde_json::Error),
    #[error("relay transaction identity failed")]
    Contract(#[from] commonkit_contracts::ContractError),
}

pub struct RelayAdapter {
    id: StableId,
    live: PathBuf,
    state: PathBuf,
    lifecycle: Option<Arc<dyn RelayLifecycleControl>>,
}

pub trait RelayLifecycleControl: Send + Sync {
    fn reload(&self, desired: &RelayConfig) -> Result<(), RelayPlanError>;
    fn restart(&self) -> Result<(), RelayPlanError>;
}

impl RelayAdapter {
    pub fn open(
        id: StableId,
        live: impl AsRef<Path>,
        state: impl AsRef<Path>,
    ) -> Result<Self, RelayPlanError> {
        fs::create_dir_all(state.as_ref())?;
        make_private(state.as_ref())?;
        let state = state.as_ref().canonicalize()?;
        Ok(Self {
            id,
            live: live.as_ref().to_owned(),
            state,
            lifecycle: None,
        })
    }

    pub fn with_lifecycle(mut self, lifecycle: Arc<dyn RelayLifecycleControl>) -> Self {
        self.lifecycle = Some(lifecycle);
        self
    }

    fn operation_path(&self, id: &Sha256Digest) -> PathBuf {
        self.state.join(format!(
            "{}.operation.json",
            id.as_str().trim_start_matches("sha256:")
        ))
    }

    fn backup_path(&self, id: &Sha256Digest) -> PathBuf {
        self.state.join(format!(
            "{}.backup.json",
            id.as_str().trim_start_matches("sha256:")
        ))
    }

    fn payload(&self, operation: &Operation) -> Result<RelayPayload, AdapterFailure> {
        let record: OperationRecord =
            read_record(&self.operation_path(&operation.id)).map_err(|_| {
                failure(
                    "relay_operation_missing",
                    "relay operation record is missing or invalid",
                )
            })?;
        let digest =
            digest_domain_json("commonkit.relay-payload.v1", &record.payload).map_err(|_| {
                failure(
                    "relay_operation_invalid",
                    "relay operation payload is invalid",
                )
            })?;
        if record.operation_id != operation.id || digest != operation.payload_digest {
            return Err(failure(
                "relay_operation_mismatch",
                "relay operation record does not match the approved plan",
            ));
        }
        Ok(record.payload)
    }
}

pub fn plan_relay_operation(
    adapter: &mut RelayAdapter,
    request: RelayPlanRequest,
) -> Result<Option<Operation>, RelayPlanError> {
    let desired_bytes = serde_jcs::to_vec(&request.desired)?;
    let after = digest_bytes(&desired_bytes)?;
    let before_bytes = read_optional_regular(&adapter.live)?;
    let before = before_bytes.as_deref().map(digest_bytes).transpose()?;
    if before.as_ref() == Some(&after) {
        return Ok(None);
    }
    let payload = RelayPayload {
        desired: request.desired,
        inputs: request.inputs,
    };
    let payload_digest = digest_domain_json("commonkit.relay-payload.v1", &payload)?;
    let operation = finalize_operation(OperationDraft {
        adapter_id: adapter.id.clone(),
        kind: if before.is_some() {
            OperationKind::Update
        } else {
            OperationKind::Create
        },
        resource: ResourceRef {
            resource_type: StableId::parse("mcp-relay")?,
            resource_id: StableId::parse("local")?,
            managed_path: Some(adapter.live.to_string_lossy().into_owned()),
        },
        risk: Risk::Medium,
        requires_confirmation: true,
        depends_on: vec![],
        before_digest: before,
        after_digest: Some(after),
        payload_digest,
        summary: "reconcile persistent MCP relay configuration".into(),
    })?;
    write_immutable(
        &adapter.operation_path(&operation.id),
        &OperationRecord {
            operation_id: operation.id.clone(),
            payload,
        },
    )?;
    Ok(Some(operation))
}

impl Adapter for RelayAdapter {
    fn id(&self) -> &StableId {
        &self.id
    }

    fn prepare(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        self.payload(operation)?;
        let bytes = read_optional_regular(&self.live).map_err(|_| {
            failure(
                "relay_inspect_failed",
                "could not inspect relay configuration",
            )
        })?;
        let digest = bytes
            .as_deref()
            .map(digest_bytes)
            .transpose()
            .map_err(|_| {
                failure(
                    "relay_inspect_failed",
                    "could not digest relay configuration",
                )
            })?;
        if digest != operation.before_digest {
            return Err(failure(
                "relay_preimage_changed",
                "relay configuration changed after planning",
            ));
        }
        write_immutable(
            &self.backup_path(&operation.id),
            &BackupRecord {
                operation_id: operation.id.clone(),
                before_digest: digest,
                bytes,
            },
        )
        .map_err(|_| failure("relay_backup_failed", "could not persist relay backup"))
    }

    fn apply(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        let payload = self.payload(operation)?;
        let bytes = serde_jcs::to_vec(&payload.desired).map_err(|_| {
            failure(
                "relay_apply_failed",
                "could not serialize relay configuration",
            )
        })?;
        atomic_replace(&self.live, &bytes).map_err(|_| {
            failure(
                "relay_apply_failed",
                "could not replace relay configuration",
            )
        })?;
        if let Some(lifecycle) = &self.lifecycle {
            lifecycle.reload(&payload.desired).map_err(|_| {
                failure("relay_reload_failed", "could not reload relay runtime")
            })?;
        }
        Ok(())
    }

    fn verify(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        let bytes = read_optional_regular(&self.live)
            .map_err(|_| {
                failure(
                    "relay_verify_failed",
                    "could not inspect relay configuration",
                )
            })?
            .ok_or_else(|| failure("relay_verify_failed", "relay configuration is missing"))?;
        let actual = digest_bytes(&bytes).map_err(|_| {
            failure(
                "relay_verify_failed",
                "could not digest relay configuration",
            )
        })?;
        if Some(actual) == operation.after_digest {
            Ok(())
        } else {
            Err(failure(
                "relay_verify_mismatch",
                "relay configuration digest differs",
            ))
        }
    }

    fn rollback(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        self.payload(operation)?;
        let backup: BackupRecord = read_record(&self.backup_path(&operation.id))
            .map_err(|_| failure("relay_backup_missing", "relay backup is missing or invalid"))?;
        if backup.operation_id != operation.id
            || backup.before_digest != operation.before_digest
            || backup
                .bytes
                .as_deref()
                .map(digest_bytes)
                .transpose()
                .ok()
                .flatten()
                != backup.before_digest
        {
            return Err(failure(
                "relay_backup_mismatch",
                "relay backup does not match the approved operation",
            ));
        }
        match backup.bytes {
            Some(bytes) => atomic_replace(&self.live, &bytes),
            None => match fs::remove_file(&self.live) {
                Ok(()) => Ok(()),
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                Err(error) => Err(error),
            },
        }
        .map_err(|_| {
            failure(
                "relay_rollback_failed",
                "could not restore relay configuration",
            )
        })?;
        if let Some(lifecycle) = &self.lifecycle {
            lifecycle.restart().map_err(|_| {
                failure("relay_rollback_failed", "could not restore relay runtime")
            })?;
        }
        Ok(())
    }
}

pub struct LegacyRelayReader;

impl LegacyRelayReader {
    pub fn read(path: impl AsRef<Path>) -> Result<RelayConfig, LegacyRelayError> {
        let bytes = read_optional_regular(path.as_ref())?.ok_or(LegacyRelayError::Missing)?;
        let value = serde_json::from_slice(&bytes)?;
        Ok(RelayConfig::normalize(value)?)
    }
}

#[derive(Debug, Error)]
pub enum LegacyRelayError {
    #[error("legacy relay configuration is missing")]
    Missing,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Config(#[from] RelayConfigError),
}

fn failure(code: &str, message: &str) -> AdapterFailure {
    AdapterFailure::new(code, message)
}

fn digest_bytes(bytes: &[u8]) -> Result<Sha256Digest, commonkit_contracts::ContractError> {
    Sha256Digest::parse(format!("sha256:{:x}", Sha256::digest(bytes)))
}

fn read_optional_regular(path: &Path) -> Result<Option<Vec<u8>>, std::io::Error> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => Err(
            std::io::Error::new(std::io::ErrorKind::InvalidInput, "unsafe relay file"),
        ),
        Ok(_) => fs::read(path).map(Some),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn write_immutable<T: Serialize>(path: &Path, value: &T) -> Result<(), RelayPlanError> {
    let bytes = serde_jcs::to_vec(value)?;
    match OpenOptions::new().create_new(true).write(true).open(path) {
        Ok(mut file) => {
            file.write_all(&bytes)?;
            file.sync_all()?;
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if read_optional_regular(path)?.as_deref() == Some(bytes.as_slice()) {
                Ok(())
            } else {
                Err(RelayPlanError::UnsafeState)
            }
        }
        Err(error) => Err(error.into()),
    }
}

fn read_record<T: for<'de> Deserialize<'de>>(path: &Path) -> Result<T, RelayPlanError> {
    let bytes = read_optional_regular(path)?.ok_or(RelayPlanError::UnsafeState)?;
    Ok(serde_json::from_slice(&bytes)?)
}

fn atomic_replace(path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let parent = path
        .parent()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::InvalidInput, "missing parent"))?;
    fs::create_dir_all(parent)?;
    let nonce = NONCE.fetch_add(1, Ordering::Relaxed);
    let temporary = parent.join(format!(
        ".commonkit-relay.{}.{}.tmp",
        std::process::id(),
        nonce
    ));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let result = (|| {
        let mut file = options.open(&temporary)?;
        file.write_all(bytes)?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

fn make_private(path: &Path) -> Result<(), std::io::Error> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}
