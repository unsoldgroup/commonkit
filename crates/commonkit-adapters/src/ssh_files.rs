use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use commonkit_contracts::{
    Operation, OperationKind, ResourceRef, Risk, Sha256Digest, StableId, digest_domain_json,
};
use commonkit_core::{OperationDraft, finalize_operation};
use commonkit_reconcile::{Adapter, AdapterFailure};
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{
    ArtifactStore, ContentReference, ContentSensitivity, FilesystemIntent, NormalizedResource,
    SafeSymlinkTarget, SshFilesystemRequest, SshFilesystemResponse, SshFilesystemTransport,
    TargetResource,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum RemotePreimage {
    Absent,
    File { digest: Sha256Digest },
    Directory { mode: Option<u32> },
    Symlink { target: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum RemoteBackup {
    Absent,
    File { content: ContentReference },
    Directory { mode: Option<u32> },
    Symlink { target: String },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct OperationRecord {
    operation_id: Sha256Digest,
    intent: FilesystemIntent,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct BackupRecord {
    operation_id: Sha256Digest,
    before_digest: Option<Sha256Digest>,
    content: Option<ContentReference>,
    #[serde(default)]
    preimage: Option<RemoteBackup>,
}

pub struct SshFileAdapter<T> {
    id: StableId,
    root_id: StableId,
    state: PathBuf,
    artifacts: ArtifactStore,
    transport: T,
    intents: BTreeMap<Sha256Digest, FilesystemIntent>,
}

impl<T: SshFilesystemTransport> SshFileAdapter<T> {
    pub fn open(
        root_id: StableId,
        state: impl AsRef<Path>,
        transport: T,
    ) -> Result<Self, SshFileAdapterError> {
        let state = state.as_ref();
        if !state.is_absolute() || state.parent().is_none() {
            return Err(SshFileAdapterError::UnsafeState);
        }
        fs::create_dir_all(state.join("operations"))?;
        fs::create_dir_all(state.join("backups"))?;
        let metadata = fs::symlink_metadata(state)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(SshFileAdapterError::UnsafeState);
        }
        Ok(Self {
            id: StableId::parse("ssh-files").expect("static stable ID"),
            root_id,
            state: state.canonicalize()?,
            artifacts: ArtifactStore::open(state.join("artifacts"))?,
            transport,
            intents: BTreeMap::new(),
        })
    }

    pub fn into_transport(self) -> T {
        self.transport
    }

    pub fn observed_state_digest<'a>(
        &mut self,
        intents: impl IntoIterator<Item = &'a FilesystemIntent>,
    ) -> Result<Sha256Digest, SshFileAdapterError> {
        let mut observed = Vec::new();
        for intent in intents {
            ensure_supported(intent)?;
            let mut preimage = self.observe(intent.path().clone())?;
            project_preimage_for_intent(intent, &mut preimage);
            observed.push((intent.path().as_str().to_owned(), preimage));
        }
        observed.sort_by(|left, right| left.0.cmp(&right.0));
        observed.dedup_by(|left, right| left.0 == right.0);
        digest_domain_json("commonkit.ssh-observed-managed-resources.v1", &observed)
            .map_err(SshFileAdapterError::Contract)
    }

    pub fn register_materialized_resource(
        &mut self,
        id: StableId,
        resource: &NormalizedResource,
        provider_artifacts: &ArtifactStore,
    ) -> Result<Option<Operation>, SshFileAdapterError> {
        let mut intent = resource.intent.clone();
        ensure_supported(&intent)?;
        if let FilesystemIntent::File { content, .. } = &mut intent {
            let bytes = provider_artifacts.load(content)?;
            if content.sensitivity != ContentSensitivity::Portable {
                return Err(SshFileAdapterError::SensitiveArtifact);
            }
            *content = self.artifacts.put(&bytes, ContentSensitivity::Portable)?;
        }
        let mut observed = self.observe(intent.path().clone())?;
        project_preimage_for_intent(&intent, &mut observed);
        let desired = desired_preimage(&intent);
        let before_digest = preimage_digest(&observed)?;
        let after_digest = preimage_digest(&desired)?;
        if before_digest == after_digest {
            return Ok(None);
        }
        let payload_digest = digest_domain_json("commonkit.ssh-filesystem-payload.v1", &intent)?;
        let kind = match (&observed, &intent) {
            (RemotePreimage::Absent, FilesystemIntent::Remove { .. }) => return Ok(None),
            (RemotePreimage::Absent, _) => OperationKind::Create,
            (_, FilesystemIntent::Remove { .. }) => OperationKind::Delete,
            _ => OperationKind::Update,
        };
        let path = intent.path().as_str().to_owned();
        let operation = finalize_operation(OperationDraft {
            adapter_id: self.id.clone(),
            kind,
            resource: ResourceRef {
                resource_type: StableId::parse(match intent {
                    FilesystemIntent::Directory { .. } => "remote-directory",
                    FilesystemIntent::Symlink { .. } => "remote-symlink",
                    FilesystemIntent::Remove { .. } => "remote-removal",
                    FilesystemIntent::File { .. } => "remote-file",
                })
                .expect("static stable ID"),
                resource_id: id,
                managed_path: Some(path.clone()),
            },
            risk: if matches!(intent, FilesystemIntent::Remove { .. }) {
                Risk::High
            } else {
                Risk::Low
            },
            requires_confirmation: true,
            depends_on: vec![],
            before_digest,
            after_digest,
            payload_digest,
            provenance: Some(resource.provenance.clone()),
            summary: format!("materialize {path} over SSH"),
        })?;
        self.write_record(
            "operations",
            &operation.id,
            &OperationRecord {
                operation_id: operation.id.clone(),
                intent: intent.clone(),
            },
        )?;
        self.intents.insert(operation.id.clone(), intent);
        Ok(Some(operation))
    }

    fn observe(
        &mut self,
        path: crate::NormalizedManagedPath,
    ) -> Result<RemotePreimage, SshFileAdapterError> {
        let inspected_path = path.clone();
        match self
            .transport
            .perform(SshFilesystemRequest::InspectResource {
                root_id: self.root_id.clone(),
                path,
            })? {
            SshFilesystemResponse::Resource { resource } => match resource {
                TargetResource::Absent => Ok(RemotePreimage::Absent),
                TargetResource::File { content } => Ok(RemotePreimage::File {
                    digest: bytes_digest(&content)?,
                }),
                TargetResource::Directory { mode } => Ok(RemotePreimage::Directory { mode }),
                TargetResource::Symlink { target } => {
                    SafeSymlinkTarget::parse(&inspected_path, target.clone())?;
                    Ok(RemotePreimage::Symlink { target })
                }
            },
            _ => Err(SshFileAdapterError::UnexpectedResponse),
        }
    }

    fn intent(&mut self, operation: &Operation) -> Result<FilesystemIntent, AdapterFailure> {
        if let Some(intent) = self.intents.get(&operation.id) {
            return Ok(intent.clone());
        }
        let record: OperationRecord =
            self.read_record("operations", &operation.id).map_err(|_| {
                failure(
                    "operation_payload_missing",
                    "remote operation payload is missing",
                )
            })?;
        let digest = digest_domain_json("commonkit.ssh-filesystem-payload.v1", &record.intent)
            .map_err(|_| {
                failure(
                    "operation_payload_mismatch",
                    "remote operation payload is invalid",
                )
            })?;
        if record.operation_id != operation.id || digest != operation.payload_digest {
            return Err(failure(
                "operation_payload_mismatch",
                "remote operation does not match its durable payload",
            ));
        }
        ensure_supported(&record.intent).map_err(|_| {
            failure(
                "unsupported_remote_resource",
                "remote resource type is unsupported",
            )
        })?;
        self.intents
            .insert(operation.id.clone(), record.intent.clone());
        Ok(record.intent)
    }

    fn write_record(
        &self,
        kind: &str,
        id: &Sha256Digest,
        value: &impl Serialize,
    ) -> Result<(), SshFileAdapterError> {
        ensure_state_dir(&self.state, kind)?;
        let path = record_path(&self.state, kind, id);
        let bytes = serde_json::to_vec(value)?;
        fs::write(path, bytes)?;
        Ok(())
    }

    fn read_record<R: for<'de> Deserialize<'de>>(
        &self,
        kind: &str,
        id: &Sha256Digest,
    ) -> Result<R, SshFileAdapterError> {
        ensure_state_dir(&self.state, kind)?;
        Ok(serde_json::from_slice(&fs::read(record_path(
            &self.state,
            kind,
            id,
        ))?)?)
    }
}

impl<T: SshFilesystemTransport + Send> Adapter for SshFileAdapter<T> {
    fn id(&self) -> &StableId {
        &self.id
    }

    fn prepare(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        let intent = self.intent(operation)?;
        let response = self
            .transport
            .perform(SshFilesystemRequest::InspectResource {
                root_id: self.root_id.clone(),
                path: intent.path().clone(),
            })
            .map_err(|_| failure("remote_inspect_failed", "could not inspect remote preimage"))?;
        let (mut observed, content, preimage) = match response {
            SshFilesystemResponse::Resource {
                resource: TargetResource::Absent,
            } => (RemotePreimage::Absent, None, RemoteBackup::Absent),
            SshFilesystemResponse::Resource {
                resource: TargetResource::File { content },
            } => {
                let digest = bytes_digest(&content).map_err(|_| {
                    failure("remote_digest_failed", "could not digest remote preimage")
                })?;
                let reference = self
                    .artifacts
                    .put(&content, ContentSensitivity::LocalSensitive)
                    .map_err(|_| failure("backup_failed", "could not persist remote preimage"))?;
                (
                    RemotePreimage::File { digest },
                    Some(reference.clone()),
                    RemoteBackup::File { content: reference },
                )
            }
            SshFilesystemResponse::Resource {
                resource: TargetResource::Directory { mode },
            } => (
                RemotePreimage::Directory { mode },
                None,
                RemoteBackup::Directory { mode },
            ),
            SshFilesystemResponse::Resource {
                resource: TargetResource::Symlink { target },
            } => (
                RemotePreimage::Symlink {
                    target: target.clone(),
                },
                None,
                RemoteBackup::Symlink { target },
            ),
            _ => {
                return Err(failure(
                    "remote_response_invalid",
                    "remote helper returned an unexpected response",
                ));
            }
        };
        project_preimage_for_intent(&intent, &mut observed);
        if preimage_digest(&observed)
            .map_err(|_| failure("remote_digest_failed", "could not digest remote preimage"))?
            != operation.before_digest
        {
            return Err(failure(
                "preimage_changed",
                "remote resource changed after planning",
            ));
        }
        self.write_record(
            "backups",
            &operation.id,
            &BackupRecord {
                operation_id: operation.id.clone(),
                before_digest: operation.before_digest.clone(),
                content,
                preimage: Some(preimage),
            },
        )
        .map_err(|_| failure("backup_failed", "could not persist remote backup record"))
    }

    fn apply(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        let intent = self.intent(operation)?;
        let request = match intent {
            FilesystemIntent::File { path, content, .. } => SshFilesystemRequest::WriteFile {
                root_id: self.root_id.clone(),
                path,
                content: self.artifacts.load(&content).map_err(|_| {
                    failure("artifact_invalid", "remote content artifact is invalid")
                })?,
            },
            FilesystemIntent::Remove { path, .. } => SshFilesystemRequest::Remove {
                root_id: self.root_id.clone(),
                path,
            },
            FilesystemIntent::Directory { path, mode, .. } => {
                SshFilesystemRequest::WriteDirectory {
                    root_id: self.root_id.clone(),
                    path,
                    mode,
                }
            }
            FilesystemIntent::Symlink { path, target, .. } => SshFilesystemRequest::WriteSymlink {
                root_id: self.root_id.clone(),
                path,
                target,
            },
        };
        match self
            .transport
            .perform(request)
            .map_err(|_| failure("remote_apply_failed", "remote helper apply failed"))?
        {
            SshFilesystemResponse::Applied => Ok(()),
            _ => Err(failure(
                "remote_response_invalid",
                "remote helper returned an unexpected response",
            )),
        }
    }

    fn verify(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        let intent = self.intent(operation)?;
        let mut observed = self
            .observe(intent.path().clone())
            .map_err(|_| failure("remote_verify_failed", "could not inspect remote result"))?;
        project_preimage_for_intent(&intent, &mut observed);
        if preimage_digest(&observed)
            .map_err(|_| failure("remote_digest_failed", "could not digest remote result"))?
            == operation.after_digest
        {
            Ok(())
        } else {
            Err(failure(
                "remote_verify_failed",
                "remote result does not match the plan",
            ))
        }
    }

    fn rollback(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        let intent = self.intent(operation)?;
        let backup: BackupRecord = self
            .read_record("backups", &operation.id)
            .map_err(|_| failure("backup_missing", "remote backup record is missing"))?;
        if backup.operation_id != operation.id || backup.before_digest != operation.before_digest {
            return Err(failure(
                "backup_mismatch",
                "remote backup does not match the plan",
            ));
        }
        let backup_preimage = backup.preimage.unwrap_or(match backup.content {
            Some(content) => RemoteBackup::File { content },
            None => RemoteBackup::Absent,
        });
        let request = match backup_preimage {
            RemoteBackup::File { content } => SshFilesystemRequest::WriteFile {
                root_id: self.root_id.clone(),
                path: intent.path().clone(),
                content: self
                    .artifacts
                    .load(&content)
                    .map_err(|_| failure("backup_invalid", "remote backup artifact is invalid"))?,
            },
            RemoteBackup::Directory { mode } => SshFilesystemRequest::WriteDirectory {
                root_id: self.root_id.clone(),
                path: intent.path().clone(),
                mode: mode.map(crate::FileMode::parse).transpose().map_err(|_| {
                    failure("backup_invalid", "remote directory backup mode is invalid")
                })?,
            },
            RemoteBackup::Symlink { target } => SshFilesystemRequest::WriteSymlink {
                root_id: self.root_id.clone(),
                path: intent.path().clone(),
                target: SafeSymlinkTarget::parse(intent.path(), target).map_err(|_| {
                    failure("backup_invalid", "remote symlink backup target is invalid")
                })?,
            },
            RemoteBackup::Absent => SshFilesystemRequest::Remove {
                root_id: self.root_id.clone(),
                path: intent.path().clone(),
            },
        };
        match self
            .transport
            .perform(request)
            .map_err(|_| failure("remote_rollback_failed", "remote rollback failed"))?
        {
            SshFilesystemResponse::Applied => {
                let mut observed = self.observe(intent.path().clone()).map_err(|_| {
                    failure("remote_rollback_failed", "could not verify remote rollback")
                })?;
                project_preimage_for_intent(&intent, &mut observed);
                if preimage_digest(&observed).map_err(|_| {
                    failure("remote_digest_failed", "could not digest rollback result")
                })? == operation.before_digest
                {
                    Ok(())
                } else {
                    Err(failure(
                        "remote_rollback_failed",
                        "remote rollback result does not match preimage",
                    ))
                }
            }
            _ => Err(failure(
                "remote_response_invalid",
                "remote helper returned an unexpected response",
            )),
        }
    }
}

fn ensure_supported(intent: &FilesystemIntent) -> Result<(), SshFileAdapterError> {
    if let FilesystemIntent::Symlink { path, target, .. } = intent {
        SafeSymlinkTarget::parse(path, target.as_str().to_owned())?;
    }
    Ok(())
}
fn desired_preimage(intent: &FilesystemIntent) -> RemotePreimage {
    match intent {
        FilesystemIntent::File { content, .. } => RemotePreimage::File {
            digest: content.digest.clone(),
        },
        FilesystemIntent::Remove { .. } => RemotePreimage::Absent,
        FilesystemIntent::Directory { mode, .. } => RemotePreimage::Directory {
            mode: mode.as_ref().map(crate::FileMode::value),
        },
        FilesystemIntent::Symlink { target, .. } => RemotePreimage::Symlink {
            target: target.as_str().to_owned(),
        },
    }
}
fn project_preimage_for_intent(intent: &FilesystemIntent, preimage: &mut RemotePreimage) {
    if matches!(intent, FilesystemIntent::Directory { mode: None, .. })
        && let RemotePreimage::Directory { mode } = preimage
    {
        *mode = None;
    }
}
fn preimage_digest(value: &RemotePreimage) -> Result<Option<Sha256Digest>, SshFileAdapterError> {
    match value {
        RemotePreimage::Absent => Ok(None),
        _ => Ok(Some(digest_domain_json(
            "commonkit.ssh-file-preimage.v1",
            value,
        )?)),
    }
}
fn bytes_digest(bytes: &[u8]) -> Result<Sha256Digest, SshFileAdapterError> {
    use sha2::{Digest, Sha256};
    Sha256Digest::parse(format!("sha256:{:x}", Sha256::digest(bytes)))
        .map_err(SshFileAdapterError::Contract)
}
fn record_path(root: &Path, kind: &str, id: &Sha256Digest) -> PathBuf {
    root.join(kind).join(format!(
        "{}.json",
        id.as_str().trim_start_matches("sha256:")
    ))
}
fn ensure_state_dir(root: &Path, kind: &str) -> Result<(), SshFileAdapterError> {
    let path = root.join(kind);
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        Err(SshFileAdapterError::UnsafeState)
    } else {
        Ok(())
    }
}
fn failure(code: &str, message: &str) -> AdapterFailure {
    AdapterFailure::new(code, message)
}

#[derive(Debug, Error)]
pub enum SshFileAdapterError {
    #[error("SSH adapter state must be an absolute non-symlink directory")]
    UnsafeState,
    #[error("SSH v1 supports normalized files and removals only")]
    UnsupportedResource,
    #[error("local-sensitive content cannot be copied to an SSH target")]
    SensitiveArtifact,
    #[error("remote helper returned an unexpected response")]
    UnexpectedResponse,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Contract(#[from] commonkit_contracts::ContractError),
    #[error(transparent)]
    Artifact(#[from] crate::ArtifactError),
    #[error(transparent)]
    Target(#[from] crate::TargetFilesystemError),
    #[error(transparent)]
    Resource(#[from] crate::ResourceError),
}
