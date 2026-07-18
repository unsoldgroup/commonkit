use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};
use commonkit_contracts::StableId;
use commonkit_core::RootAccess;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::NormalizedManagedPath;

/// Filesystem operations available to target adapters after a root capability is granted.
pub trait TargetFilesystem {
    fn read_file(
        &self,
        path: &NormalizedManagedPath,
    ) -> Result<Option<Vec<u8>>, TargetFilesystemError>;

    fn write_file(
        &self,
        path: &NormalizedManagedPath,
        content: &[u8],
    ) -> Result<(), TargetFilesystemError>;
}

pub struct LocalTargetFilesystem {
    root: Dir,
    access: RootAccess,
}

impl LocalTargetFilesystem {
    pub fn open(root: &Path, access: RootAccess) -> Result<Self, TargetFilesystemError> {
        if !root.is_absolute() || root.parent().is_none() {
            return Err(TargetFilesystemError::InvalidRoot);
        }
        let metadata = std::fs::symlink_metadata(root)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(TargetFilesystemError::InvalidRoot);
        }
        Ok(Self {
            root: Dir::open_ambient_dir(root, ambient_authority())?,
            access,
        })
    }

    fn ensure_safe_ancestors(
        &self,
        path: &NormalizedManagedPath,
        create: bool,
    ) -> Result<(), TargetFilesystemError> {
        let components = path.as_str().split('/').collect::<Vec<_>>();
        let mut parent = PathBuf::new();
        for component in components.iter().take(components.len().saturating_sub(1)) {
            parent.push(component);
            match self.root.symlink_metadata(&parent) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(TargetFilesystemError::SymlinkEncountered(
                        parent.to_string_lossy().into_owned(),
                    ));
                }
                Ok(metadata) if !metadata.is_dir() => {
                    return Err(TargetFilesystemError::NotDirectory(
                        parent.to_string_lossy().into_owned(),
                    ));
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound && create => {
                    self.root.create_dir(&parent)?;
                }
                Err(error) => return Err(error.into()),
            }
        }
        Ok(())
    }

    fn reject_symlink_leaf(
        &self,
        path: &NormalizedManagedPath,
    ) -> Result<bool, TargetFilesystemError> {
        match self.root.symlink_metadata(path.as_str()) {
            Ok(metadata) if metadata.file_type().is_symlink() => {
                Err(TargetFilesystemError::SymlinkEncountered(path.to_string()))
            }
            Ok(metadata) if !metadata.is_file() => {
                Err(TargetFilesystemError::NotFile(path.to_string()))
            }
            Ok(_) => Ok(true),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
            Err(error) => Err(error.into()),
        }
    }
}

impl TargetFilesystem for LocalTargetFilesystem {
    fn read_file(
        &self,
        path: &NormalizedManagedPath,
    ) -> Result<Option<Vec<u8>>, TargetFilesystemError> {
        self.ensure_safe_ancestors(path, false)?;
        if !self.reject_symlink_leaf(path)? {
            return Ok(None);
        }
        let mut file = self.root.open(path.as_str())?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(Some(bytes))
    }

    fn write_file(
        &self,
        path: &NormalizedManagedPath,
        content: &[u8],
    ) -> Result<(), TargetFilesystemError> {
        if self.access != RootAccess::ReadWrite {
            return Err(TargetFilesystemError::ReadOnly);
        }
        self.ensure_safe_ancestors(path, true)?;
        self.reject_symlink_leaf(path)?;
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        let mut file = self.root.open_with(path.as_str(), &options)?;
        file.write_all(content)?;
        file.sync_all()?;
        Ok(())
    }
}

/// A deliberately closed SSH protocol. Implementations can map these requests to SFTP or a
/// constrained helper, but cannot accept arbitrary shell commands.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "operation", rename_all = "snake_case", deny_unknown_fields)]
pub enum SshFilesystemRequest {
    ReadFile {
        root_id: StableId,
        path: NormalizedManagedPath,
    },
    WriteFile {
        root_id: StableId,
        path: NormalizedManagedPath,
        content: Vec<u8>,
    },
    Remove {
        root_id: StableId,
        path: NormalizedManagedPath,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum SshFilesystemResponse {
    Absent,
    File { content: Vec<u8> },
    Applied,
}

pub trait SshFilesystemTransport {
    fn perform(
        &mut self,
        request: SshFilesystemRequest,
    ) -> Result<SshFilesystemResponse, TargetFilesystemError>;
}

#[derive(Debug, Error)]
pub enum TargetFilesystemError {
    #[error("target capability root must be an absolute, real directory")]
    InvalidRoot,
    #[error("target root is read-only")]
    ReadOnly,
    #[error("symlink encountered in managed target path: {0}")]
    SymlinkEncountered(String),
    #[error("managed target path component is not a directory: {0}")]
    NotDirectory(String),
    #[error("managed target path is not a regular file: {0}")]
    NotFile(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
