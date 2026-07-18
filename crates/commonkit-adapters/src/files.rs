use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};
use commonkit_contracts::{
    Operation, OperationKind, ResourceRef, Risk, Sha256Digest, StableId,
    assert_no_embedded_secrets, canonical_json, digest_domain_json,
};
use commonkit_core::{OperationDraft, finalize_operation};
use commonkit_reconcile::{Adapter, AdapterFailure};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{ArtifactStore, ContentReference, ContentSensitivity};

static REPLACEMENT_NONCE: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct ManagedRelativePath(PathBuf);

impl ManagedRelativePath {
    pub fn parse(value: impl AsRef<Path>) -> Result<Self, FileAdapterError> {
        let value = value.as_ref();
        let portable = value.to_string_lossy();
        let valid = !value.as_os_str().is_empty()
            && !portable.contains('\\')
            && !portable.contains(':')
            && !portable
                .split('/')
                .any(|segment| segment.is_empty() || segment == ".git")
            && value
                .components()
                .all(|component| matches!(component, Component::Normal(_)));
        if valid {
            Ok(Self(value.to_path_buf()))
        } else {
            Err(FileAdapterError::UnsafePath)
        }
    }

    pub fn as_path(&self) -> &Path {
        &self.0
    }

    fn portable(&self) -> String {
        self.0
            .components()
            .map(|component| component.as_os_str().to_string_lossy())
            .collect::<Vec<_>>()
            .join("/")
    }
}

#[derive(Debug, Clone)]
pub struct FileIntent {
    pub id: StableId,
    pub path: ManagedRelativePath,
    pub content: Vec<u8>,
    pub expected_before: Option<Sha256Digest>,
}

#[derive(Debug, Clone)]
struct ManagedFile {
    path: ManagedRelativePath,
    content: ContentReference,
    expected_before: Option<Sha256Digest>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ManagedFilePayload {
    path: String,
    content: ContentReference,
    expected_before: Option<Sha256Digest>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ManagedFileRecord {
    operation_id: Sha256Digest,
    payload: ManagedFilePayload,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BackupRecord {
    operation_id: Sha256Digest,
    before_digest: Option<Sha256Digest>,
    content: Option<ContentReference>,
}

pub struct FileAdapter {
    id: StableId,
    target: Dir,
    state: Dir,
    artifacts: ArtifactStore,
    files: BTreeMap<Sha256Digest, ManagedFile>,
}

impl FileAdapter {
    pub fn open(target: &Path, state: &Path) -> Result<Self, FileAdapterError> {
        std::fs::create_dir_all(target)?;
        std::fs::create_dir_all(state)?;
        std::fs::create_dir_all(state.join("operations"))?;
        std::fs::create_dir_all(state.join("backups"))?;
        let artifacts = ArtifactStore::open(state.join("artifacts"))?;
        Ok(Self {
            id: StableId::parse("files").expect("static stable ID"),
            target: Dir::open_ambient_dir(target, ambient_authority())?,
            state: Dir::open_ambient_dir(state, ambient_authority())?,
            artifacts,
            files: BTreeMap::new(),
        })
    }

    pub fn register(&mut self, intent: FileIntent) -> Result<Operation, FileAdapterError> {
        if let Ok(text) = std::str::from_utf8(&intent.content) {
            assert_no_embedded_secrets(&Value::String(text.into()))?;
        }
        let observed = read_optional(&self.target, intent.path.as_path())?;
        let before_digest = observed.as_deref().map(digest_bytes).transpose()?;
        if intent.expected_before != before_digest {
            return Err(FileAdapterError::PreimageMismatch);
        }
        let after_digest = digest_bytes(&intent.content)?;
        if before_digest == Some(after_digest.clone()) {
            return Err(FileAdapterError::NoChange);
        }
        let kind = if observed.is_some() {
            OperationKind::Update
        } else {
            OperationKind::Create
        };
        let content = self
            .artifacts
            .put(&intent.content, ContentSensitivity::Portable)?;
        let payload = ManagedFilePayload {
            path: intent.path.portable(),
            content: content.clone(),
            expected_before: intent.expected_before.clone(),
        };
        let payload_digest = digest_domain_json("commonkit.filesystem-payload.v1", &payload)?;
        let operation = finalize_operation(OperationDraft {
            adapter_id: self.id.clone(),
            kind,
            resource: ResourceRef {
                resource_type: StableId::parse("file").expect("static stable ID"),
                resource_id: intent.id,
                managed_path: Some(intent.path.portable()),
            },
            risk: Risk::Low,
            requires_confirmation: true,
            depends_on: Vec::new(),
            before_digest,
            after_digest: Some(after_digest),
            payload_digest,
            summary: format!("materialize {}", intent.path.portable()),
        })?;
        let record = ManagedFileRecord {
            operation_id: operation.id.clone(),
            payload,
        };
        write_record(&self.state, &operation_record_path(&operation.id), &record)?;
        self.files.insert(
            operation.id.clone(),
            ManagedFile {
                path: intent.path,
                content,
                expected_before: intent.expected_before,
            },
        );
        Ok(operation)
    }

    fn managed(&mut self, operation: &Operation) -> Result<ManagedFile, AdapterFailure> {
        if let Some(managed) = self.files.get(&operation.id) {
            return Ok(managed.clone());
        }
        let record: ManagedFileRecord =
            read_record(&self.state, &operation_record_path(&operation.id)).map_err(|_| {
                failure(
                    "operation_payload_missing",
                    "durable operation payload is missing or invalid",
                )
            })?;
        let payload_digest = digest_domain_json("commonkit.filesystem-payload.v1", &record.payload)
            .map_err(|_| {
                failure(
                    "operation_payload_mismatch",
                    "durable operation payload cannot be verified",
                )
            })?;
        if record.operation_id != operation.id
            || payload_digest != operation.payload_digest
            || operation.resource.managed_path.as_deref() != Some(record.payload.path.as_str())
            || operation.after_digest.as_ref() != Some(&record.payload.content.digest)
            || operation.before_digest != record.payload.expected_before
        {
            return Err(failure(
                "operation_payload_mismatch",
                "durable operation payload does not match the plan",
            ));
        }
        let managed = ManagedFile {
            path: ManagedRelativePath::parse(record.payload.path).map_err(|_| {
                failure(
                    "operation_payload_mismatch",
                    "durable operation path is invalid",
                )
            })?,
            content: record.payload.content,
            expected_before: record.payload.expected_before,
        };
        self.artifacts.load(&managed.content).map_err(|_| {
            failure(
                "artifact_invalid",
                "operation content artifact is missing or invalid",
            )
        })?;
        self.files.insert(operation.id.clone(), managed.clone());
        Ok(managed)
    }
}

impl Adapter for FileAdapter {
    fn id(&self) -> &StableId {
        &self.id
    }

    fn prepare(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        let managed = self.managed(operation)?;
        let observed = read_optional(&self.target, managed.path.as_path())
            .map_err(|_| failure("inspect_failed", "could not inspect managed file"))?;
        let observed_digest = observed
            .as_deref()
            .map(digest_bytes)
            .transpose()
            .map_err(|_| failure("digest_failed", "could not digest managed file"))?;
        if observed_digest != operation.before_digest || observed_digest != managed.expected_before
        {
            return Err(failure(
                "preimage_changed",
                "managed file changed after planning",
            ));
        }
        let content = observed
            .as_deref()
            .map(|bytes| {
                self.artifacts
                    .put(bytes, ContentSensitivity::LocalSensitive)
            })
            .transpose()
            .map_err(|_| failure("backup_failed", "could not persist preimage artifact"))?;
        write_record(
            &self.state,
            &backup_record_path(&operation.id),
            &BackupRecord {
                operation_id: operation.id.clone(),
                before_digest: operation.before_digest.clone(),
                content,
            },
        )
        .map_err(|_| failure("backup_failed", "could not persist preimage backup record"))?;
        Ok(())
    }

    fn apply(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        let managed = self.managed(operation)?;
        let content = self.artifacts.load(&managed.content).map_err(|_| {
            failure(
                "artifact_invalid",
                "operation content artifact is missing or invalid",
            )
        })?;
        replace_file(&self.target, managed.path.as_path(), &content)
            .map_err(|_| failure("apply_failed", "could not replace managed file"))
    }

    fn verify(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        let managed = self.managed(operation)?;
        let actual = read_optional(&self.target, managed.path.as_path())
            .map_err(|_| failure("verify_failed", "could not read managed file"))?
            .ok_or_else(|| failure("verify_failed", "managed file is missing"))?;
        let digest = digest_bytes(&actual)
            .map_err(|_| failure("verify_failed", "could not digest managed file"))?;
        if Some(digest) == operation.after_digest {
            Ok(())
        } else {
            Err(failure("verify_mismatch", "managed file digest differs"))
        }
    }

    fn rollback(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        let managed = self.managed(operation)?;
        let backup: BackupRecord = read_record(&self.state, &backup_record_path(&operation.id))
            .map_err(|_| {
                failure(
                    "rollback_backup_missing",
                    "preimage backup record is missing",
                )
            })?;
        if backup.operation_id != operation.id || backup.before_digest != operation.before_digest {
            return Err(failure(
                "rollback_backup_mismatch",
                "preimage backup record does not match the operation",
            ));
        }
        match backup.content {
            Some(reference) => {
                let bytes = self.artifacts.load(&reference).map_err(|_| {
                    failure(
                        "rollback_backup_mismatch",
                        "preimage backup artifact is missing or invalid",
                    )
                })?;
                if Some(reference.digest) != operation.before_digest {
                    return Err(failure(
                        "rollback_backup_mismatch",
                        "preimage backup digest does not match the operation",
                    ));
                }
                replace_file(&self.target, managed.path.as_path(), &bytes)
            }
            None if operation.kind == OperationKind::Create
                && operation.before_digest.is_none() =>
            {
                match self.target.remove_file(managed.path.as_path()) {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
                    Err(error) => Err(error),
                }
            }
            None => {
                return Err(failure(
                    "rollback_backup_missing",
                    "preimage backup is missing",
                ));
            }
        }
        .map_err(|_| failure("rollback_failed", "could not restore managed file"))
    }
}

fn read_optional(directory: &Dir, path: &Path) -> Result<Option<Vec<u8>>, std::io::Error> {
    match directory.symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "managed file path is not an ordinary file",
            ));
        }
        Ok(_) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    }
    match directory.open(path) {
        Ok(mut file) => {
            let mut bytes = Vec::new();
            file.read_to_end(&mut bytes)?;
            Ok(Some(bytes))
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn write_new(directory: &Dir, path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    let mut file = directory.open_with(path, &options)?;
    file.write_all(bytes)?;
    file.sync_all()
}

fn write_record<T: Serialize>(
    directory: &Dir,
    path: &Path,
    value: &T,
) -> Result<(), FileAdapterError> {
    let bytes = canonical_json(value)?;
    match write_new(directory, path, &bytes) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if read_optional(directory, path)?.as_deref() == Some(bytes.as_slice()) {
                Ok(())
            } else {
                Err(FileAdapterError::Io(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "durable record path contains different content",
                )))
            }
        }
        Err(error) => Err(error.into()),
    }
}

fn read_record<T: for<'de> Deserialize<'de>>(
    directory: &Dir,
    path: &Path,
) -> Result<T, FileAdapterError> {
    let metadata = directory.symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_file() {
        return Err(FileAdapterError::UnsafeStateEntry);
    }
    let bytes = read_optional(directory, path)?.ok_or(FileAdapterError::MissingStateEntry)?;
    serde_json::from_slice(&bytes).map_err(FileAdapterError::Serialization)
}

fn operation_record_path(operation_id: &Sha256Digest) -> PathBuf {
    PathBuf::from("operations").join(format!(
        "{}.json",
        operation_id.as_str().trim_start_matches("sha256:")
    ))
}

fn backup_record_path(operation_id: &Sha256Digest) -> PathBuf {
    PathBuf::from("backups").join(format!(
        "{}.json",
        operation_id.as_str().trim_start_matches("sha256:")
    ))
}

fn replace_file(directory: &Dir, path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let file_name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("missing filename"))?;
    let nonce = REPLACEMENT_NONCE.fetch_add(1, Ordering::Relaxed);
    let temporary = path.with_file_name(format!(
        ".{}.commonkit-tmp-{}-{nonce}",
        file_name.to_string_lossy(),
        std::process::id()
    ));
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        directory.create_dir_all(parent)?;
    }
    write_new(directory, &temporary, bytes)?;
    let result = directory.rename(&temporary, directory, path);
    if result.is_err() {
        let _ = directory.remove_file(&temporary);
    }
    result
}

fn digest_bytes(bytes: &[u8]) -> Result<Sha256Digest, commonkit_contracts::ContractError> {
    let digest = Sha256::digest(bytes);
    Sha256Digest::parse(format!("sha256:{digest:x}"))
}

fn failure(code: &str, message: &str) -> AdapterFailure {
    AdapterFailure::new(code, message)
}

#[derive(Debug, Error)]
pub enum FileAdapterError {
    #[error("managed path must be a non-empty relative path without traversal")]
    UnsafePath,
    #[error("file intent does not change observed state")]
    NoChange,
    #[error("file intent preimage does not match observed target state")]
    PreimageMismatch,
    #[error("durable state entry is missing")]
    MissingStateEntry,
    #[error("durable state entry is not an ordinary file")]
    UnsafeStateEntry,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Artifact(#[from] crate::ArtifactError),
    #[error(transparent)]
    Serialization(#[from] serde_json::Error),
    #[error(transparent)]
    Contract(#[from] commonkit_contracts::ContractError),
}
