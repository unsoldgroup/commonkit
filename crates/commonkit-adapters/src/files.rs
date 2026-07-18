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
use crate::{FileMode, FilesystemIntent, NormalizedManagedPath, SafeSymlinkTarget};

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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SemanticRecord {
    operation_id: Sha256Digest,
    intent: FilesystemIntent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum ResourcePreimage {
    Absent,
    File {
        content: ContentReference,
        mode: Option<u32>,
    },
    Directory {
        mode: Option<u32>,
    },
    Symlink {
        target: String,
    },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct SemanticBackupRecord {
    operation_id: Sha256Digest,
    before_digest: Option<Sha256Digest>,
    preimage: ResourcePreimage,
}

pub struct FileAdapter {
    id: StableId,
    target_root: PathBuf,
    target: Dir,
    state: Dir,
    artifacts: ArtifactStore,
    files: BTreeMap<Sha256Digest, ManagedFile>,
    resources: BTreeMap<Sha256Digest, FilesystemIntent>,
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
            target_root: target.to_path_buf(),
            target: Dir::open_ambient_dir(target, ambient_authority())?,
            state: Dir::open_ambient_dir(state, ambient_authority())?,
            artifacts,
            files: BTreeMap::new(),
            resources: BTreeMap::new(),
        })
    }

    /// Computes a canonical digest of the live state for the exact normalized
    /// resources a provider intends to manage. Inspection stays capability-
    /// rooted and follows the same semantic representation as prepare/verify.
    pub fn observed_state_digest<'a>(
        &self,
        intents: impl IntoIterator<Item = &'a FilesystemIntent>,
    ) -> Result<Sha256Digest, FileAdapterError> {
        let mut observed = Vec::new();
        for intent in intents {
            validate_semantic_intent(intent)?;
            observed.push((
                intent.path().as_str().to_owned(),
                inspect_resource(&self.target, &self.artifacts, intent.path().as_str())?,
            ));
        }
        observed.sort_by(|left, right| left.0.cmp(&right.0));
        observed.dedup_by(|left, right| left.0 == right.0);
        digest_domain_json("commonkit.observed-managed-resources.v1", &observed)
            .map_err(FileAdapterError::Contract)
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

    pub fn register_resource(
        &mut self,
        id: StableId,
        intent: FilesystemIntent,
    ) -> Result<Operation, FileAdapterError> {
        validate_semantic_intent(&intent)?;
        let observed = inspect_resource(&self.target, &self.artifacts, intent.path().as_str())?;
        let before_digest = semantic_digest(&observed)?;
        let expected = match &intent {
            FilesystemIntent::Symlink {
                expected_before, ..
            }
            | FilesystemIntent::Remove {
                expected_before, ..
            } => expected_before,
            _ => &None,
        };
        if expected.is_some() && expected != &before_digest {
            return Err(FileAdapterError::PreimageMismatch);
        }
        let desired = desired_preimage(&intent);
        let after_digest = semantic_digest(&desired)?;
        if before_digest == after_digest {
            return Err(FileAdapterError::NoChange);
        }
        let payload_digest =
            digest_domain_json("commonkit.filesystem-resource-payload.v1", &intent)?;
        let kind = match (&observed, &intent) {
            (ResourcePreimage::Absent, FilesystemIntent::Remove { .. }) => {
                return Err(FileAdapterError::NoChange);
            }
            (ResourcePreimage::Absent, _) => OperationKind::Create,
            (_, FilesystemIntent::Remove { .. }) => OperationKind::Delete,
            _ => OperationKind::Update,
        };
        let path = intent.path().as_str().to_owned();
        let resource_type = match intent {
            FilesystemIntent::Directory { .. } => "directory",
            FilesystemIntent::Symlink { .. } => "symlink",
            FilesystemIntent::Remove { .. } => "removal",
            FilesystemIntent::File { .. } => "filesystem-file",
        };
        let operation = finalize_operation(OperationDraft {
            adapter_id: self.id.clone(),
            kind,
            resource: ResourceRef {
                resource_type: StableId::parse(resource_type).expect("static ID"),
                resource_id: id,
                managed_path: Some(path.clone()),
            },
            risk: if matches!(intent, FilesystemIntent::Remove { .. }) {
                Risk::High
            } else {
                Risk::Low
            },
            requires_confirmation: true,
            depends_on: Vec::new(),
            before_digest,
            after_digest,
            payload_digest,
            summary: format!("materialize {path}"),
        })?;
        write_record(
            &self.state,
            &operation_record_path(&operation.id),
            &SemanticRecord {
                operation_id: operation.id.clone(),
                intent: intent.clone(),
            },
        )?;
        self.resources.insert(operation.id.clone(), intent);
        Ok(operation)
    }

    /// Registers provider output after copying and verifying its content in the
    /// adapter's durable artifact store. Planning never writes the live target.
    pub fn register_materialized_resource(
        &mut self,
        id: StableId,
        mut intent: FilesystemIntent,
        provider_artifacts: &ArtifactStore,
    ) -> Result<Operation, FileAdapterError> {
        if let FilesystemIntent::File { content, .. } = &mut intent {
            let bytes = provider_artifacts.load(content)?;
            *content = self.artifacts.put(&bytes, content.sensitivity)?;
        }
        self.register_resource(id, intent)
    }

    fn semantic(&mut self, operation: &Operation) -> Result<FilesystemIntent, AdapterFailure> {
        if let Some(intent) = self.resources.get(&operation.id) {
            return Ok(intent.clone());
        }
        let record: SemanticRecord =
            read_record(&self.state, &operation_record_path(&operation.id)).map_err(|_| {
                failure(
                    "operation_payload_missing",
                    "durable resource payload is missing or invalid",
                )
            })?;
        validate_semantic_intent(&record.intent).map_err(|_| {
            failure(
                "operation_payload_mismatch",
                "durable resource payload is unsafe",
            )
        })?;
        let digest = digest_domain_json("commonkit.filesystem-resource-payload.v1", &record.intent)
            .map_err(|_| {
                failure(
                    "operation_payload_mismatch",
                    "durable resource payload cannot be verified",
                )
            })?;
        if record.operation_id != operation.id
            || digest != operation.payload_digest
            || operation.resource.managed_path.as_deref() != Some(record.intent.path().as_str())
        {
            return Err(failure(
                "operation_payload_mismatch",
                "durable resource payload does not match the plan",
            ));
        }
        self.resources
            .insert(operation.id.clone(), record.intent.clone());
        Ok(record.intent)
    }

    fn is_semantic(operation: &Operation) -> bool {
        matches!(
            operation.resource.resource_type.as_str(),
            "filesystem-file" | "directory" | "symlink" | "removal"
        )
    }
}

impl FileAdapter {
    fn prepare_semantic(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        let intent = self.semantic(operation)?;
        let observed = inspect_resource(&self.target, &self.artifacts, intent.path().as_str())
            .map_err(|_| failure("inspect_failed", "could not inspect managed resource"))?;
        let digest = semantic_digest(&observed)
            .map_err(|_| failure("digest_failed", "could not digest managed resource"))?;
        if digest != operation.before_digest {
            return Err(failure(
                "preimage_changed",
                "managed resource changed after planning",
            ));
        }
        let durable = persist_preimage(&self.artifacts, observed)
            .map_err(|_| failure("backup_failed", "could not persist resource preimage"))?;
        write_record(
            &self.state,
            &backup_record_path(&operation.id),
            &SemanticBackupRecord {
                operation_id: operation.id.clone(),
                before_digest: operation.before_digest.clone(),
                preimage: durable,
            },
        )
        .map_err(|_| failure("backup_failed", "could not persist resource backup record"))
    }

    fn apply_semantic(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        let intent = self.semantic(operation)?;
        validate_no_symlink_ancestors(&self.target_root, intent.path().as_str())
            .map_err(|_| failure("unsafe_path", "managed resource has a symlink ancestor"))?;
        let path = self.target_root.join(intent.path().as_str());
        if let FilesystemIntent::File { content, mode, .. } = &intent {
            let bytes = self.artifacts.load(content).map_err(|_| {
                failure(
                    "artifact_invalid",
                    "resource content artifact is missing or invalid",
                )
            })?;
            remove_entry(&path)
                .map_err(|_| failure("apply_failed", "could not clear managed resource"))?;
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|_| failure("apply_failed", "could not create resource parent"))?;
            }
            std::fs::write(&path, bytes)
                .map_err(|_| failure("apply_failed", "could not materialize managed file"))?;
            set_mode(&path, mode.as_ref().map(FileMode::value))
                .map_err(|_| failure("apply_failed", "could not set managed file mode"))?;
            return Ok(());
        }
        apply_intent(&path, &intent)
            .map_err(|_| failure("apply_failed", "could not materialize managed resource"))
    }

    fn verify_semantic(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        let intent = self.semantic(operation)?;
        let mut actual = inspect_resource(&self.target, &self.artifacts, intent.path().as_str())
            .map_err(|_| failure("verify_failed", "could not inspect managed resource"))?;
        if matches!(
            &intent,
            FilesystemIntent::File { mode: None, .. }
                | FilesystemIntent::Directory { mode: None, .. }
        ) {
            match &mut actual {
                ResourcePreimage::File { mode, .. } | ResourcePreimage::Directory { mode } => {
                    *mode = None;
                }
                _ => {}
            }
        }
        let digest = semantic_digest(&actual)
            .map_err(|_| failure("verify_failed", "could not digest managed resource"))?;
        if digest == operation.after_digest {
            Ok(())
        } else {
            Err(failure("verify_mismatch", "managed resource differs"))
        }
    }

    fn rollback_semantic(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        let intent = self.semantic(operation)?;
        let backup: SemanticBackupRecord =
            read_record(&self.state, &backup_record_path(&operation.id)).map_err(|_| {
                failure(
                    "rollback_backup_missing",
                    "resource backup record is missing",
                )
            })?;
        if backup.operation_id != operation.id || backup.before_digest != operation.before_digest {
            return Err(failure(
                "rollback_backup_mismatch",
                "resource backup does not match operation",
            ));
        }
        let preimage = load_preimage(&self.artifacts, backup.preimage).map_err(|_| {
            failure(
                "rollback_backup_mismatch",
                "resource backup artifact is invalid",
            )
        })?;
        if semantic_digest(&preimage).map_err(|_| {
            failure(
                "rollback_backup_mismatch",
                "resource backup cannot be digested",
            )
        })? != operation.before_digest
        {
            return Err(failure(
                "rollback_backup_mismatch",
                "resource backup digest differs",
            ));
        }
        validate_no_symlink_ancestors(&self.target_root, intent.path().as_str())
            .map_err(|_| failure("unsafe_path", "managed resource has a symlink ancestor"))?;
        let restore_path = self.target_root.join(intent.path().as_str());
        if let ResourcePreimage::File { content, mode } = &preimage {
            remove_entry(&restore_path)
                .map_err(|_| failure("rollback_failed", "could not clear managed resource"))?;
            let bytes = self.artifacts.load(content).map_err(|_| {
                failure(
                    "rollback_backup_mismatch",
                    "resource backup artifact is invalid",
                )
            })?;
            if let Some(parent) = restore_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|_| failure("rollback_failed", "could not create resource parent"))?;
            }
            std::fs::write(&restore_path, bytes)
                .map_err(|_| failure("rollback_failed", "could not restore managed file"))?;
            set_mode(&restore_path, *mode)
                .map_err(|_| failure("rollback_failed", "could not restore managed file mode"))?;
            return Ok(());
        }
        restore_preimage(&restore_path, &preimage)
            .map_err(|_| failure("rollback_failed", "could not restore managed resource"))
    }
}

impl Adapter for FileAdapter {
    fn id(&self) -> &StableId {
        &self.id
    }

    fn prepare(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        if Self::is_semantic(operation) {
            return self.prepare_semantic(operation);
        }
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
        if Self::is_semantic(operation) {
            return self.apply_semantic(operation);
        }
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
        if Self::is_semantic(operation) {
            return self.verify_semantic(operation);
        }
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
        if Self::is_semantic(operation) {
            return self.rollback_semantic(operation);
        }
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

fn validate_semantic_intent(intent: &FilesystemIntent) -> Result<(), FileAdapterError> {
    let path = NormalizedManagedPath::parse(intent.path().as_str().to_owned())?;
    if let FilesystemIntent::Symlink { target, .. } = intent {
        SafeSymlinkTarget::parse(&path, target.as_str().to_owned())?;
    }
    Ok(())
}

fn desired_preimage(intent: &FilesystemIntent) -> ResourcePreimage {
    match intent {
        FilesystemIntent::Directory { mode, .. } => ResourcePreimage::Directory {
            mode: mode.as_ref().map(FileMode::value),
        },
        FilesystemIntent::Symlink { target, .. } => ResourcePreimage::Symlink {
            target: target.as_str().to_owned(),
        },
        FilesystemIntent::Remove { .. } => ResourcePreimage::Absent,
        FilesystemIntent::File { content, mode, .. } => ResourcePreimage::File {
            content: content.clone(),
            mode: mode.as_ref().map(FileMode::value),
        },
    }
}

fn semantic_digest(
    value: &ResourcePreimage,
) -> Result<Option<Sha256Digest>, commonkit_contracts::ContractError> {
    match value {
        ResourcePreimage::Absent => Ok(None),
        ResourcePreimage::File { content, mode } => digest_domain_json(
            "commonkit.filesystem-resource-state.v1",
            &("file", &content.digest, mode),
        )
        .map(Some),
        ResourcePreimage::Directory { mode } => digest_domain_json(
            "commonkit.filesystem-resource-state.v1",
            &("directory", mode),
        )
        .map(Some),
        ResourcePreimage::Symlink { target } => digest_domain_json(
            "commonkit.filesystem-resource-state.v1",
            &("symlink", target),
        )
        .map(Some),
    }
}

fn inspect_resource(
    directory: &Dir,
    artifacts: &ArtifactStore,
    path: &str,
) -> Result<ResourcePreimage, FileAdapterError> {
    let path = Path::new(path);
    let metadata = match directory.symlink_metadata(path) {
        Ok(value) => value,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Ok(ResourcePreimage::Absent);
        }
        Err(error) => return Err(error.into()),
    };
    if metadata.file_type().is_symlink() {
        let target = directory.read_link(path)?;
        return Ok(ResourcePreimage::Symlink {
            target: target.to_string_lossy().into_owned(),
        });
    }
    let mode = portable_mode(&metadata);
    if metadata.is_dir() {
        return Ok(ResourcePreimage::Directory { mode });
    }
    if metadata.is_file() {
        let bytes = read_optional(directory, path)?.ok_or(FileAdapterError::MissingStateEntry)?;
        let content = artifacts.put(&bytes, ContentSensitivity::LocalSensitive)?;
        return Ok(ResourcePreimage::File { content, mode });
    }
    Err(FileAdapterError::UnsupportedResource)
}

#[cfg(unix)]
fn portable_mode(metadata: &cap_std::fs::Metadata) -> Option<u32> {
    use cap_std::fs::MetadataExt;
    Some(metadata.mode() & 0o7777)
}
#[cfg(not(unix))]
fn portable_mode(_metadata: &cap_std::fs::Metadata) -> Option<u32> {
    None
}

fn persist_preimage(
    artifacts: &ArtifactStore,
    value: ResourcePreimage,
) -> Result<ResourcePreimage, FileAdapterError> {
    if let ResourcePreimage::File { content, .. } = &value {
        artifacts.load(content)?;
    }
    Ok(value)
}
fn load_preimage(
    artifacts: &ArtifactStore,
    value: ResourcePreimage,
) -> Result<ResourcePreimage, FileAdapterError> {
    persist_preimage(artifacts, value)
}

fn validate_no_symlink_ancestors(root: &Path, relative: &str) -> Result<(), std::io::Error> {
    let mut current = root.to_path_buf();
    let components: Vec<_> = Path::new(relative).components().collect();
    for component in components.iter().take(components.len().saturating_sub(1)) {
        current.push(component.as_os_str());
        match std::fs::symlink_metadata(&current) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "symlink ancestor",
                ));
            }
            Ok(metadata) if !metadata.is_dir() => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "non-directory ancestor",
                ));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn remove_entry(path: &Path) -> Result<(), std::io::Error> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) if metadata.file_type().is_symlink() || metadata.is_file() => {
            std::fs::remove_file(path)
        }
        Ok(metadata) if metadata.is_dir() => std::fs::remove_dir(path),
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "unsupported resource type",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn apply_intent(path: &Path, intent: &FilesystemIntent) -> Result<(), std::io::Error> {
    match intent {
        FilesystemIntent::Directory { mode, .. } => {
            match std::fs::symlink_metadata(path) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
                Ok(_) => {
                    remove_entry(path)?;
                    std::fs::create_dir_all(path)?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    std::fs::create_dir_all(path)?
                }
                Err(error) => return Err(error),
            }
            set_mode(path, mode.as_ref().map(FileMode::value))
        }
        FilesystemIntent::Symlink { target, .. } => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            remove_entry(path)?;
            create_symlink(target.as_str(), path)
        }
        FilesystemIntent::Remove { .. } => remove_entry(path),
        FilesystemIntent::File { content, mode, .. } => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            remove_entry(path)?;
            // The reference was integrity-checked and copied during registration;
            // actual bytes are written by `apply_semantic`, which has store access.
            let _ = (content, mode);
            Err(std::io::Error::new(
                std::io::ErrorKind::Unsupported,
                "file content requires adapter artifact access",
            ))
        }
    }
}

fn restore_preimage(path: &Path, preimage: &ResourcePreimage) -> Result<(), std::io::Error> {
    remove_entry(path)?;
    match preimage {
        ResourcePreimage::Absent => Ok(()),
        ResourcePreimage::Directory { mode } => {
            std::fs::create_dir_all(path)?;
            set_mode(path, *mode)
        }
        ResourcePreimage::Symlink { target } => {
            if let Some(parent) = path.parent() {
                std::fs::create_dir_all(parent)?;
            }
            create_symlink(target, path)
        }
        ResourcePreimage::File { .. } => Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "file artifact requires adapter access",
        )),
    }
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: Option<u32>) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(mode) = mode {
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}
#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: Option<u32>) -> Result<(), std::io::Error> {
    Ok(())
}

#[cfg(unix)]
fn create_symlink(target: &str, path: &Path) -> Result<(), std::io::Error> {
    std::os::unix::fs::symlink(target, path)
}
#[cfg(windows)]
fn create_symlink(target: &str, path: &Path) -> Result<(), std::io::Error> {
    std::os::windows::fs::symlink_file(target, path)
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
    #[error("filesystem resource type is unsupported")]
    UnsupportedResource,
    #[error(transparent)]
    Resource(#[from] crate::ResourceError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Artifact(#[from] crate::ArtifactError),
    #[error(transparent)]
    Serialization(#[from] serde_json::Error),
    #[error(transparent)]
    Contract(#[from] commonkit_contracts::ContractError),
}
