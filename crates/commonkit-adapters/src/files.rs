use std::collections::BTreeMap;
use std::io::{Read, Write};
use std::path::{Component, Path, PathBuf};

use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};
use commonkit_contracts::{
    Operation, OperationKind, ResourceRef, Risk, Sha256Digest, StableId, assert_no_embedded_secrets,
};
use commonkit_core::{OperationDraft, finalize_operation};
use commonkit_reconcile::{Adapter, AdapterFailure};
use serde_json::Value;
use sha2::{Digest, Sha256};
use thiserror::Error;

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

struct ManagedFile {
    path: ManagedRelativePath,
    content: Vec<u8>,
    expected_before: Option<Sha256Digest>,
    backup_name: String,
}

pub struct FileAdapter {
    id: StableId,
    target: Dir,
    state: Dir,
    files: BTreeMap<Sha256Digest, ManagedFile>,
}

impl FileAdapter {
    pub fn open(target: &Path, state: &Path) -> Result<Self, FileAdapterError> {
        std::fs::create_dir_all(target)?;
        std::fs::create_dir_all(state)?;
        Ok(Self {
            id: StableId::parse("files").expect("static stable ID"),
            target: Dir::open_ambient_dir(target, ambient_authority())?,
            state: Dir::open_ambient_dir(state, ambient_authority())?,
            files: BTreeMap::new(),
        })
    }

    pub fn register(&mut self, intent: FileIntent) -> Result<Operation, FileAdapterError> {
        if let Ok(text) = std::str::from_utf8(&intent.content) {
            assert_no_embedded_secrets(&Value::String(text.into()))?;
        }
        let observed = read_optional(&self.target, intent.path.as_path())?;
        let before_digest = observed.as_deref().map(digest_bytes).transpose()?;
        let after_digest = digest_bytes(&intent.content)?;
        if before_digest == Some(after_digest.clone()) {
            return Err(FileAdapterError::NoChange);
        }
        let kind = if observed.is_some() {
            OperationKind::Update
        } else {
            OperationKind::Create
        };
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
            summary: format!("materialize {}", intent.path.portable()),
        })?;
        let backup_name = format!(
            "{}.backup",
            operation.id.as_str().trim_start_matches("sha256:")
        );
        self.files.insert(
            operation.id.clone(),
            ManagedFile {
                path: intent.path,
                content: intent.content,
                expected_before: intent.expected_before,
                backup_name,
            },
        );
        Ok(operation)
    }

    fn managed(&self, operation: &Operation) -> Result<&ManagedFile, AdapterFailure> {
        self.files
            .get(&operation.id)
            .ok_or_else(|| failure("unknown_operation", "operation was not registered"))
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
        if let Some(bytes) = observed {
            write_backup(&self.state, Path::new(&managed.backup_name), &bytes)
                .map_err(|_| failure("backup_failed", "could not persist preimage backup"))?;
        }
        Ok(())
    }

    fn apply(&mut self, operation: &Operation) -> Result<(), AdapterFailure> {
        let managed = self.managed(operation)?;
        replace_file(&self.target, managed.path.as_path(), &managed.content)
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
        match read_optional(&self.state, Path::new(&managed.backup_name))
            .map_err(|_| failure("rollback_failed", "could not read preimage backup"))?
        {
            Some(bytes) => replace_file(&self.target, managed.path.as_path(), &bytes),
            None if operation.kind == OperationKind::Create => {
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

fn write_backup(directory: &Dir, path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    match write_new(directory, path, bytes) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            if read_optional(directory, path)?.as_deref() == Some(bytes) {
                Ok(())
            } else {
                Err(std::io::Error::new(
                    std::io::ErrorKind::AlreadyExists,
                    "backup path contains different content",
                ))
            }
        }
        Err(error) => Err(error),
    }
}

fn replace_file(directory: &Dir, path: &Path, bytes: &[u8]) -> Result<(), std::io::Error> {
    let file_name = path
        .file_name()
        .ok_or_else(|| std::io::Error::other("missing filename"))?;
    let temporary = path.with_file_name(format!(
        ".{}.commonkit-tmp-{}",
        file_name.to_string_lossy(),
        std::process::id()
    ));
    if directory.symlink_metadata(&temporary).is_ok() {
        directory.remove_file(&temporary)?;
    }
    if let Some(parent) = path.parent()
        && !parent.as_os_str().is_empty()
    {
        directory.create_dir_all(parent)?;
    }
    write_new(directory, &temporary, bytes)?;
    if directory.symlink_metadata(path).is_ok() {
        directory.remove_file(path)?;
    }
    directory.rename(&temporary, directory, path)
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
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Contract(#[from] commonkit_contracts::ContractError),
}
