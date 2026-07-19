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
    target_identity: DirectoryIdentity,
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
        let target_directory = open_directory_handle(target)?;
        let target_identity = directory_identity(&target_directory)?;
        Ok(Self {
            id: StableId::parse("files").expect("static stable ID"),
            target_root: target.to_path_buf(),
            target: target_directory,
            target_identity,
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

    /// Reconstructs the normalized intents referenced by durable operations and
    /// observes their live state without relying on process-local registration.
    pub fn observed_operations_digest(
        &self,
        operations: &[Operation],
    ) -> Result<Sha256Digest, FileAdapterError> {
        let mut intents = Vec::with_capacity(operations.len());
        for operation in operations {
            let record: SemanticRecord =
                read_record(&self.state, &operation_record_path(&operation.id))?;
            if record.operation_id != operation.id {
                return Err(FileAdapterError::MissingStateEntry);
            }
            intents.push(record.intent);
        }
        self.observed_state_digest(intents.iter())
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
        self.validate_target_binding()
            .map_err(|_| failure("unsafe_path", "managed target root was substituted"))?;
        let observed = inspect_resource(&self.target, &self.artifacts, intent.path().as_str())
            .map_err(|_| failure("unsafe_path", "managed resource path is unsafe"))?;
        let observed_digest = semantic_digest(&observed)
            .map_err(|_| failure("preimage_changed", "managed resource cannot be digested"))?;
        if observed_digest != operation.before_digest {
            return Err(failure(
                "preimage_changed",
                "managed resource changed after prepare",
            ));
        }
        let create_parent = !matches!(intent, FilesystemIntent::Remove { .. });
        let (parent, leaf) =
            open_parent_nofollow(&self.target, intent.path().as_str(), create_parent)
                .map_err(|_| failure("unsafe_path", "managed resource has an unsafe ancestor"))?;
        if let FilesystemIntent::File { content, mode, .. } = &intent {
            let bytes = self.artifacts.load(content).map_err(|_| {
                failure(
                    "artifact_invalid",
                    "resource content artifact is missing or invalid",
                )
            })?;
            replace_file_in(&parent, &leaf, &bytes, mode.as_ref().map(FileMode::value))
                .map_err(|_| failure("apply_failed", "could not materialize managed file"))?;
            return Ok(());
        }
        apply_intent_in(&parent, &leaf, &intent)
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
        self.validate_target_binding()
            .map_err(|_| failure("unsafe_path", "managed target root was substituted"))?;
        let mut current = inspect_resource(&self.target, &self.artifacts, intent.path().as_str())
            .map_err(|_| failure("unsafe_path", "managed rollback path is unsafe"))?;
        if matches!(
            &intent,
            FilesystemIntent::File { mode: None, .. }
                | FilesystemIntent::Directory { mode: None, .. }
        ) {
            match &mut current {
                ResourcePreimage::File { mode, .. } | ResourcePreimage::Directory { mode } => {
                    *mode = None;
                }
                _ => {}
            }
        }
        let current_digest = semantic_digest(&current).map_err(|_| {
            failure(
                "rollback_preimage_changed",
                "managed rollback state cannot be digested",
            )
        })?;
        if current_digest != operation.after_digest && !matches!(current, ResourcePreimage::Absent)
        {
            return Err(failure(
                "rollback_preimage_changed",
                "managed resource changed after apply",
            ));
        }
        let create_parent = !matches!(preimage, ResourcePreimage::Absent);
        let (parent, leaf) =
            open_parent_nofollow(&self.target, intent.path().as_str(), create_parent)
                .map_err(|_| failure("unsafe_path", "managed resource has an unsafe ancestor"))?;
        if let ResourcePreimage::File { content, mode } = &preimage {
            let bytes = self.artifacts.load(content).map_err(|_| {
                failure(
                    "rollback_backup_mismatch",
                    "resource backup artifact is invalid",
                )
            })?;
            replace_file_in(&parent, &leaf, &bytes, *mode)
                .map_err(|_| failure("rollback_failed", "could not restore managed file"))?;
            return Ok(());
        }
        restore_preimage_in(&parent, &leaf, &preimage)
            .map_err(|_| failure("rollback_failed", "could not restore managed resource"))
    }

    fn validate_target_binding(&self) -> Result<(), std::io::Error> {
        let current = open_directory_handle(&self.target_root)?;
        if directory_identity(&current)? == self.target_identity {
            Ok(())
        } else {
            Err(std::io::Error::other(
                "managed target root identity changed",
            ))
        }
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
        self.validate_target_binding()
            .map_err(|_| failure("unsafe_path", "managed target root was substituted"))?;
        let (parent, leaf) = open_parent_nofollow(&self.target, &managed.path.portable(), true)
            .map_err(|_| failure("unsafe_path", "managed file has an unsafe ancestor"))?;
        let observed = read_optional_in(&parent, &leaf)
            .map_err(|_| failure("unsafe_path", "managed file path is unsafe"))?;
        let observed_digest = observed
            .as_deref()
            .map(digest_bytes)
            .transpose()
            .map_err(|_| failure("preimage_changed", "managed file cannot be digested"))?;
        if observed_digest != operation.before_digest {
            return Err(failure(
                "preimage_changed",
                "managed file changed after prepare",
            ));
        }
        replace_file_in(&parent, &leaf, &content, None)
            .map_err(|_| failure("apply_failed", "could not replace managed file"))
    }

    fn verify(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        if Self::is_semantic(operation) {
            return self.verify_semantic(operation);
        }
        let managed = self.managed(operation)?;
        self.validate_target_binding()
            .map_err(|_| failure("unsafe_path", "managed target root was substituted"))?;
        let (parent, leaf) = open_parent_nofollow(&self.target, &managed.path.portable(), false)
            .map_err(|_| failure("unsafe_path", "managed file has an unsafe ancestor"))?;
        let actual = read_optional_in(&parent, &leaf)
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
        self.validate_target_binding()
            .map_err(|_| failure("unsafe_path", "managed target root was substituted"))?;
        let (parent, leaf) = open_parent_nofollow(
            &self.target,
            &managed.path.portable(),
            backup.content.is_some(),
        )
        .map_err(|_| failure("unsafe_path", "managed file has an unsafe ancestor"))?;
        let current = read_optional_in(&parent, &leaf)
            .map_err(|_| failure("unsafe_path", "managed rollback path is unsafe"))?;
        let current_digest = current
            .as_deref()
            .map(digest_bytes)
            .transpose()
            .map_err(|_| {
                failure(
                    "rollback_preimage_changed",
                    "managed rollback state cannot be digested",
                )
            })?;
        if current_digest != operation.after_digest && current.is_some() {
            return Err(failure(
                "rollback_preimage_changed",
                "managed file changed after apply",
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
                replace_file_in(&parent, &leaf, &bytes, None)
            }
            None if operation.kind == OperationKind::Create
                && operation.before_digest.is_none() =>
            {
                remove_entry_in(&parent, &leaf)
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

#[derive(Debug, Clone, PartialEq, Eq)]
struct DirectoryIdentity {
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
    #[cfg(windows)]
    volume: Option<u32>,
    #[cfg(windows)]
    index: Option<u64>,
    #[cfg(not(any(unix, windows)))]
    modified: Option<std::time::SystemTime>,
}

fn directory_identity(directory: &Dir) -> Result<DirectoryIdentity, std::io::Error> {
    let metadata = directory.try_clone()?.into_std_file().metadata()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        Ok(DirectoryIdentity {
            device: metadata.dev(),
            inode: metadata.ino(),
        })
    }
    #[cfg(windows)]
    {
        use std::os::windows::fs::MetadataExt;
        Ok(DirectoryIdentity {
            volume: metadata.volume_serial_number(),
            index: metadata.file_index(),
        })
    }
    #[cfg(not(any(unix, windows)))]
    {
        Ok(DirectoryIdentity {
            modified: metadata.modified().ok(),
        })
    }
}

fn open_parent_nofollow(
    root: &Dir,
    relative: &str,
    create: bool,
) -> Result<(Dir, PathBuf), std::io::Error> {
    let path = Path::new(relative);
    let leaf = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("missing managed resource name"))?
        .into();
    let mut current = root.try_clone()?;
    if let Some(parent) = path.parent() {
        for component in parent.components() {
            let Component::Normal(name) = component else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "unsafe managed path component",
                ));
            };
            match open_dir_component_nofollow(&current, Path::new(name)) {
                Ok(next) => current = next,
                Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
                    match current.create_dir(name) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(error) => return Err(error),
                    }
                    current = open_dir_component_nofollow(&current, Path::new(name))?;
                }
                Err(error) => return Err(error),
            }
        }
    }
    Ok((current, leaf))
}

fn open_dir_component_nofollow(parent: &Dir, name: &Path) -> Result<Dir, std::io::Error> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use cap_std::fs::OpenOptionsExt;
        const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let file = parent.open_with(name, &options)?;
    let metadata = file.metadata()?;
    if metadata_is_reparse_or_symlink(&metadata) || !metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "managed path component is not a directory",
        ));
    }
    Ok(Dir::from_std_file(file.into_std()))
}

#[cfg(unix)]
fn open_directory_handle(path: &Path) -> Result<Dir, std::io::Error> {
    use std::fs::OpenOptions as StdOpenOptions;
    use std::os::unix::fs::OpenOptionsExt;

    let file = StdOpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)?;
    Ok(Dir::from_std_file(file))
}

#[cfg(windows)]
fn open_directory_handle(path: &Path) -> Result<Dir, std::io::Error> {
    use std::fs::OpenOptions as StdOpenOptions;
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    let file = StdOpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)?;
    let metadata = file.metadata()?;
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    if metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
        || !metadata.is_dir()
    {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "managed root is not an ordinary directory",
        ));
    }
    Ok(Dir::from_std_file(file))
}

#[cfg(not(any(unix, windows)))]
fn open_directory_handle(path: &Path) -> Result<Dir, std::io::Error> {
    Dir::open_ambient_dir(path, ambient_authority())
}

fn remove_entry_in(parent: &Dir, leaf: &Path) -> Result<(), std::io::Error> {
    match parent.symlink_metadata(leaf) {
        Ok(metadata) if metadata.file_type().is_symlink() || metadata.is_file() => {
            parent.remove_file(leaf)
        }
        Ok(metadata) if metadata.is_dir() => parent.remove_dir(leaf),
        Ok(_) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "unsupported resource type",
        )),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn replace_file_in(
    parent: &Dir,
    leaf: &Path,
    bytes: &[u8],
    mode: Option<u32>,
) -> Result<(), std::io::Error> {
    if let Ok(metadata) = parent.symlink_metadata(leaf)
        && metadata.is_dir()
        && !metadata.file_type().is_symlink()
    {
        parent.remove_dir(leaf)?;
    }
    let nonce = REPLACEMENT_NONCE.fetch_add(1, Ordering::Relaxed);
    let temporary = PathBuf::from(format!(
        ".{}.commonkit-tmp-{}-{nonce}",
        leaf.to_string_lossy(),
        std::process::id()
    ));
    let mut options = OpenOptions::new();
    options.create_new(true).write(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
        options.mode(mode.unwrap_or(0o600));
    }
    #[cfg(windows)]
    {
        use cap_std::fs::OpenOptionsExt;
        use windows_sys::Win32::Foundation::GENERIC_WRITE;
        use windows_sys::Win32::Storage::FileSystem::{
            DELETE, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
        };
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.access_mode(GENERIC_WRITE | DELETE);
        options.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = parent.open_with(&temporary, &options)?;
    let result = (|| {
        file.write_all(bytes)?;
        set_file_mode(&file, mode)?;
        file.sync_all()?;
        atomic_replace_in(parent, &temporary, leaf, &file)
    })();
    if result.is_err() {
        let _ = parent.remove_file(&temporary);
    }
    result
}

#[cfg(not(windows))]
fn atomic_replace_in(
    parent: &Dir,
    temporary: &Path,
    leaf: &Path,
    _temporary_file: &cap_std::fs::File,
) -> Result<(), std::io::Error> {
    parent.rename(temporary, parent, leaf)
}

#[cfg(windows)]
fn atomic_replace_in(
    parent: &Dir,
    _temporary: &Path,
    leaf: &Path,
    temporary_file: &cap_std::fs::File,
) -> Result<(), std::io::Error> {
    use std::mem::{offset_of, size_of};
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_RENAME_INFO, FileRenameInfo, SetFileInformationByHandle,
    };

    let name = leaf.as_os_str().encode_wide().collect::<Vec<_>>();
    let name_bytes = name
        .len()
        .checked_mul(size_of::<u16>())
        .ok_or_else(|| std::io::Error::other("replacement filename is too long"))?;
    let total_bytes = offset_of!(FILE_RENAME_INFO, FileName)
        .checked_add(name_bytes)
        .ok_or_else(|| std::io::Error::other("replacement filename is too long"))?;
    let words = total_bytes.div_ceil(size_of::<usize>());
    let mut storage = vec![0usize; words];
    let info = storage.as_mut_ptr().cast::<FILE_RENAME_INFO>();
    let parent_file = parent.try_clone()?.into_std_file();
    unsafe {
        (*info).Anonymous.ReplaceIfExists = 1;
        (*info).RootDirectory = parent_file.as_raw_handle() as _;
        (*info).FileNameLength = u32::try_from(name_bytes)
            .map_err(|_| std::io::Error::other("replacement filename is too long"))?;
        std::ptr::copy_nonoverlapping(
            name.as_ptr(),
            std::ptr::addr_of_mut!((*info).FileName).cast::<u16>(),
            name.len(),
        );
        let result = SetFileInformationByHandle(
            temporary_file.as_raw_handle() as _,
            FileRenameInfo,
            info.cast(),
            u32::try_from(total_bytes)
                .map_err(|_| std::io::Error::other("replacement metadata is too large"))?,
        );
        if result == 0 {
            return Err(std::io::Error::last_os_error());
        }
    }
    Ok(())
}

#[cfg(windows)]
fn metadata_is_reparse_or_symlink(metadata: &cap_std::fs::Metadata) -> bool {
    use cap_std::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn metadata_is_reparse_or_symlink(metadata: &cap_std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

fn apply_intent_in(
    parent: &Dir,
    leaf: &Path,
    intent: &FilesystemIntent,
) -> Result<(), std::io::Error> {
    match intent {
        FilesystemIntent::Directory { mode, .. } => {
            match parent.symlink_metadata(leaf) {
                Ok(metadata) if metadata.is_dir() && !metadata.file_type().is_symlink() => {}
                Ok(_) => {
                    remove_entry_in(parent, leaf)?;
                    parent.create_dir(leaf)?;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    parent.create_dir(leaf)?;
                }
                Err(error) => return Err(error),
            }
            let directory = open_dir_component_nofollow(parent, leaf)?;
            set_directory_mode(&directory, mode.as_ref().map(FileMode::value))
        }
        FilesystemIntent::Symlink { target, .. } => {
            remove_entry_in(parent, leaf)?;
            create_symlink_in(parent, target.as_str(), leaf)
        }
        FilesystemIntent::Remove { .. } => remove_entry_in(parent, leaf),
        FilesystemIntent::File { .. } => Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "file content requires adapter artifact access",
        )),
    }
}

fn restore_preimage_in(
    parent: &Dir,
    leaf: &Path,
    preimage: &ResourcePreimage,
) -> Result<(), std::io::Error> {
    remove_entry_in(parent, leaf)?;
    match preimage {
        ResourcePreimage::Absent => Ok(()),
        ResourcePreimage::Directory { mode } => {
            parent.create_dir(leaf)?;
            let directory = open_dir_component_nofollow(parent, leaf)?;
            set_directory_mode(&directory, *mode)
        }
        ResourcePreimage::Symlink { target } => create_symlink_in(parent, target, leaf),
        ResourcePreimage::File { .. } => Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "file artifact requires adapter access",
        )),
    }
}

#[cfg(unix)]
fn set_file_mode(file: &cap_std::fs::File, mode: Option<u32>) -> Result<(), std::io::Error> {
    use cap_std::fs::PermissionsExt;
    if let Some(mode) = mode {
        file.set_permissions(cap_std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_file_mode(_file: &cap_std::fs::File, _mode: Option<u32>) -> Result<(), std::io::Error> {
    Ok(())
}

#[cfg(unix)]
fn set_directory_mode(directory: &Dir, mode: Option<u32>) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt;
    if let Some(mode) = mode {
        directory
            .try_clone()?
            .into_std_file()
            .set_permissions(std::fs::Permissions::from_mode(mode))?;
    }
    Ok(())
}

#[cfg(not(unix))]
fn set_directory_mode(_directory: &Dir, _mode: Option<u32>) -> Result<(), std::io::Error> {
    Ok(())
}

#[cfg(not(windows))]
fn create_symlink_in(parent: &Dir, target: &str, leaf: &Path) -> Result<(), std::io::Error> {
    parent.symlink(target, leaf)
}

#[cfg(windows)]
fn create_symlink_in(parent: &Dir, target: &str, leaf: &Path) -> Result<(), std::io::Error> {
    parent.symlink_file(target, leaf)
}

fn read_optional(directory: &Dir, path: &Path) -> Result<Option<Vec<u8>>, std::io::Error> {
    match directory.symlink_metadata(path) {
        Ok(metadata) if metadata_is_reparse_or_symlink(&metadata) || !metadata.is_file() => {
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

fn read_optional_in(parent: &Dir, leaf: &Path) -> Result<Option<Vec<u8>>, std::io::Error> {
    let metadata = match parent.symlink_metadata(leaf) {
        Ok(metadata) if metadata_is_reparse_or_symlink(&metadata) || !metadata.is_file() => {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "managed file leaf is not an ordinary file",
            ));
        }
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error),
    };
    if !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "managed file leaf is not an ordinary file",
        ));
    }
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    {
        use cap_std::fs::OpenOptionsExt;
        const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
        options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
    }
    let mut file = parent.open_with(leaf, &options)?;
    let metadata = file.metadata()?;
    if metadata_is_reparse_or_symlink(&metadata) || !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "managed file leaf is not an ordinary file",
        ));
    }
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(Some(bytes))
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
