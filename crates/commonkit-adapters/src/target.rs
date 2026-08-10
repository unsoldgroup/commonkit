use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};
use commonkit_contracts::StableId;
use commonkit_core::RootAccess;
use serde::{Deserialize, Serialize};
use thiserror::Error;

use crate::{FileMode, NormalizedManagedPath, SafeSymlinkTarget, SymlinkTargetKind};

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

    fn remove(&self, path: &NormalizedManagedPath) -> Result<(), TargetFilesystemError>;

    fn inspect_resource(
        &self,
        path: &NormalizedManagedPath,
    ) -> Result<TargetResource, TargetFilesystemError>;

    fn write_directory(
        &self,
        path: &NormalizedManagedPath,
        mode: Option<&FileMode>,
    ) -> Result<(), TargetFilesystemError>;

    fn write_symlink(
        &self,
        path: &NormalizedManagedPath,
        target: &SafeSymlinkTarget,
        target_kind: SymlinkTargetKind,
    ) -> Result<(), TargetFilesystemError>;

    fn remove_resource(&self, path: &NormalizedManagedPath) -> Result<(), TargetFilesystemError>;
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum TargetResource {
    Absent,
    File {
        content: Vec<u8>,
    },
    Directory {
        mode: Option<u32>,
    },
    Symlink {
        target: String,
        #[serde(default, skip_serializing_if = "SymlinkTargetKind::is_file")]
        target_kind: SymlinkTargetKind,
    },
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
        if std_metadata_is_reparse_or_symlink(&metadata) || !metadata.is_dir() {
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
                Ok(metadata) if metadata_is_reparse_or_symlink(&metadata) => {
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
            Ok(metadata) if metadata_is_reparse_or_symlink(&metadata) => {
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

    fn remove(&self, path: &NormalizedManagedPath) -> Result<(), TargetFilesystemError> {
        if self.access != RootAccess::ReadWrite {
            return Err(TargetFilesystemError::ReadOnly);
        }
        self.ensure_safe_ancestors(path, false)?;
        if self.reject_symlink_leaf(path)? {
            self.root.remove_file(path.as_str())?;
        }
        Ok(())
    }

    fn inspect_resource(
        &self,
        path: &NormalizedManagedPath,
    ) -> Result<TargetResource, TargetFilesystemError> {
        let (parent, leaf) = match open_target_parent_nofollow(&self.root, path, false) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(TargetResource::Absent);
            }
            Err(error) => return Err(error.into()),
        };
        let metadata = match parent.symlink_metadata(&leaf) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(TargetResource::Absent);
            }
            Err(error) => return Err(error.into()),
        };
        if metadata.file_type().is_symlink() {
            let target = parent.read_link(&leaf)?.to_string_lossy().into_owned();
            return Ok(TargetResource::Symlink {
                target_kind: inspect_symlink_target_kind(&self.root, path, &target)?,
                target,
            });
        }
        if metadata_is_reparse_or_symlink(&metadata) {
            return Err(TargetFilesystemError::UnsupportedResource(path.to_string()));
        }
        if metadata.is_dir() {
            return Ok(TargetResource::Directory {
                mode: target_directory_mode(&metadata),
            });
        }
        if metadata.is_file() {
            let mut file = open_target_file_nofollow(&parent, &leaf)?;
            let mut content = Vec::new();
            file.read_to_end(&mut content)?;
            return Ok(TargetResource::File { content });
        }
        Err(TargetFilesystemError::UnsupportedResource(path.to_string()))
    }

    fn write_directory(
        &self,
        path: &NormalizedManagedPath,
        mode: Option<&FileMode>,
    ) -> Result<(), TargetFilesystemError> {
        if self.access != RootAccess::ReadWrite {
            return Err(TargetFilesystemError::ReadOnly);
        }
        #[cfg(not(unix))]
        if mode.is_some() {
            return Err(TargetFilesystemError::UnsupportedMode);
        }
        let (parent, leaf) = open_target_parent_nofollow(&self.root, path, true)?;
        match parent.symlink_metadata(&leaf) {
            Ok(metadata) if metadata.is_dir() && !metadata_is_reparse_or_symlink(&metadata) => {}
            Ok(_) => remove_target_entry(&parent, &leaf)?,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(error.into()),
        }
        if !matches!(parent.symlink_metadata(&leaf), Ok(metadata) if metadata.is_dir()) {
            parent.create_dir(&leaf)?;
        }
        let directory = open_target_dir_nofollow(&parent, &leaf)?;
        set_target_directory_mode(&directory, mode.map(FileMode::value))?;
        Ok(())
    }

    fn write_symlink(
        &self,
        path: &NormalizedManagedPath,
        target: &SafeSymlinkTarget,
        target_kind: SymlinkTargetKind,
    ) -> Result<(), TargetFilesystemError> {
        if self.access != RootAccess::ReadWrite {
            return Err(TargetFilesystemError::ReadOnly);
        }
        SafeSymlinkTarget::parse(path, target.as_str().to_owned())?;
        let (parent, leaf) = open_target_parent_nofollow(&self.root, path, true)?;
        remove_target_entry(&parent, &leaf)?;
        create_target_symlink(&parent, target.as_str(), &leaf, target_kind)?;
        Ok(())
    }

    fn remove_resource(&self, path: &NormalizedManagedPath) -> Result<(), TargetFilesystemError> {
        if self.access != RootAccess::ReadWrite {
            return Err(TargetFilesystemError::ReadOnly);
        }
        let (parent, leaf) = match open_target_parent_nofollow(&self.root, path, false) {
            Ok(value) => value,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        remove_target_entry(&parent, &leaf)?;
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
    InspectResource {
        root_id: StableId,
        path: NormalizedManagedPath,
    },
    WriteFile {
        root_id: StableId,
        path: NormalizedManagedPath,
        content: Vec<u8>,
    },
    WriteDirectory {
        root_id: StableId,
        path: NormalizedManagedPath,
        mode: Option<FileMode>,
    },
    WriteSymlink {
        root_id: StableId,
        path: NormalizedManagedPath,
        target: SafeSymlinkTarget,
        #[serde(default, skip_serializing_if = "SymlinkTargetKind::is_file")]
        target_kind: SymlinkTargetKind,
    },
    Remove {
        root_id: StableId,
        path: NormalizedManagedPath,
    },
    StageArtifact {
        run_id: StableId,
        digest: commonkit_contracts::Sha256Digest,
        content: Vec<u8>,
    },
    VerifyArtifact {
        run_id: StableId,
        digest: commonkit_contracts::Sha256Digest,
    },
    BindRecoveryReceipt {
        run_id: StableId,
        receipt_digest: commonkit_contracts::Sha256Digest,
    },
    RecoverRun {
        run_id: StableId,
        receipt_digest: commonkit_contracts::Sha256Digest,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "snake_case", deny_unknown_fields)]
pub enum SshFilesystemResponse {
    Absent,
    File {
        content: Vec<u8>,
    },
    Resource {
        resource: TargetResource,
    },
    Applied,
    ArtifactStaged {
        digest: commonkit_contracts::Sha256Digest,
    },
    ArtifactVerified {
        digest: commonkit_contracts::Sha256Digest,
    },
    RecoveryReceiptBound {
        receipt_digest: commonkit_contracts::Sha256Digest,
    },
    RecoveryReady {
        receipt_digest: commonkit_contracts::Sha256Digest,
    },
}

pub trait SshFilesystemTransport {
    fn perform(
        &mut self,
        request: SshFilesystemRequest,
    ) -> Result<SshFilesystemResponse, TargetFilesystemError>;
}

impl<T: SshFilesystemTransport + ?Sized> SshFilesystemTransport for Box<T> {
    fn perform(
        &mut self,
        request: SshFilesystemRequest,
    ) -> Result<SshFilesystemResponse, TargetFilesystemError> {
        (**self).perform(request)
    }
}

pub struct SshTargetFilesystem<T> {
    root_id: StableId,
    transport: Mutex<T>,
}

impl<T> SshTargetFilesystem<T> {
    pub fn new(root_id: StableId, transport: T) -> Self {
        Self {
            root_id,
            transport: Mutex::new(transport),
        }
    }

    pub fn into_transport(self) -> Result<T, TargetFilesystemError> {
        self.transport
            .into_inner()
            .map_err(|_| TargetFilesystemError::InvalidRemoteResponse)
    }
}

impl<T: SshFilesystemTransport + Send> TargetFilesystem for SshTargetFilesystem<T> {
    fn read_file(
        &self,
        path: &NormalizedManagedPath,
    ) -> Result<Option<Vec<u8>>, TargetFilesystemError> {
        match self
            .transport
            .lock()
            .map_err(|_| TargetFilesystemError::InvalidRemoteResponse)?
            .perform(SshFilesystemRequest::ReadFile {
                root_id: self.root_id.clone(),
                path: path.clone(),
            })? {
            SshFilesystemResponse::Absent => Ok(None),
            SshFilesystemResponse::File { content } => Ok(Some(content)),
            _ => Err(TargetFilesystemError::InvalidRemoteResponse),
        }
    }

    fn write_file(
        &self,
        path: &NormalizedManagedPath,
        content: &[u8],
    ) -> Result<(), TargetFilesystemError> {
        match self
            .transport
            .lock()
            .map_err(|_| TargetFilesystemError::InvalidRemoteResponse)?
            .perform(SshFilesystemRequest::WriteFile {
                root_id: self.root_id.clone(),
                path: path.clone(),
                content: content.to_vec(),
            })? {
            SshFilesystemResponse::Applied => Ok(()),
            _ => Err(TargetFilesystemError::InvalidRemoteResponse),
        }
    }

    fn remove(&self, path: &NormalizedManagedPath) -> Result<(), TargetFilesystemError> {
        match self
            .transport
            .lock()
            .map_err(|_| TargetFilesystemError::InvalidRemoteResponse)?
            .perform(SshFilesystemRequest::Remove {
                root_id: self.root_id.clone(),
                path: path.clone(),
            })? {
            SshFilesystemResponse::Applied => Ok(()),
            _ => Err(TargetFilesystemError::InvalidRemoteResponse),
        }
    }

    fn inspect_resource(
        &self,
        path: &NormalizedManagedPath,
    ) -> Result<TargetResource, TargetFilesystemError> {
        match self
            .transport
            .lock()
            .map_err(|_| TargetFilesystemError::InvalidRemoteResponse)?
            .perform(SshFilesystemRequest::InspectResource {
                root_id: self.root_id.clone(),
                path: path.clone(),
            })? {
            SshFilesystemResponse::Resource { resource } => Ok(resource),
            _ => Err(TargetFilesystemError::InvalidRemoteResponse),
        }
    }

    fn write_directory(
        &self,
        path: &NormalizedManagedPath,
        mode: Option<&FileMode>,
    ) -> Result<(), TargetFilesystemError> {
        match self
            .transport
            .lock()
            .map_err(|_| TargetFilesystemError::InvalidRemoteResponse)?
            .perform(SshFilesystemRequest::WriteDirectory {
                root_id: self.root_id.clone(),
                path: path.clone(),
                mode: mode.cloned(),
            })? {
            SshFilesystemResponse::Applied => Ok(()),
            _ => Err(TargetFilesystemError::InvalidRemoteResponse),
        }
    }

    fn write_symlink(
        &self,
        path: &NormalizedManagedPath,
        target: &SafeSymlinkTarget,
        target_kind: SymlinkTargetKind,
    ) -> Result<(), TargetFilesystemError> {
        match self
            .transport
            .lock()
            .map_err(|_| TargetFilesystemError::InvalidRemoteResponse)?
            .perform(SshFilesystemRequest::WriteSymlink {
                root_id: self.root_id.clone(),
                path: path.clone(),
                target: target.clone(),
                target_kind,
            })? {
            SshFilesystemResponse::Applied => Ok(()),
            _ => Err(TargetFilesystemError::InvalidRemoteResponse),
        }
    }

    fn remove_resource(&self, path: &NormalizedManagedPath) -> Result<(), TargetFilesystemError> {
        self.remove(path)
    }
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
    #[error("managed target directory contains an unsafe entry")]
    InvalidDirectoryEntry,
    #[error("managed target path has an unsupported resource type: {0}")]
    UnsupportedResource(String),
    #[error("target platform cannot apply portable file modes")]
    UnsupportedMode,
    #[error("invalid SSH target configuration: {0}")]
    InvalidSshConfig(&'static str),
    #[error("remote host key does not match the configured fingerprint")]
    HostKeyMismatch,
    #[error(
        "multiple target host keys match the configured fingerprint; remove duplicate known_hosts entries"
    )]
    AmbiguousHostKeyPin,
    #[error("remote helper failed ({status}): {message}")]
    RemoteFailure { status: i32, message: String },
    #[error("remote helper returned an invalid or unexpected response")]
    InvalidRemoteResponse,
    #[error("remote helper does not recognize target capability root: {0}")]
    UnknownRoot(StableId),
    #[error("remote artifact validation failed")]
    RemoteArtifact,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Resource(#[from] crate::ResourceError),
}

fn open_target_parent_nofollow(
    root: &Dir,
    path: &NormalizedManagedPath,
    create: bool,
) -> Result<(Dir, PathBuf), std::io::Error> {
    let relative = Path::new(path.as_str());
    let leaf = relative
        .file_name()
        .ok_or_else(|| std::io::Error::other("missing managed resource name"))?
        .into();
    let mut current = root.try_clone()?;
    if let Some(parent) = relative.parent() {
        for component in parent.components() {
            let std::path::Component::Normal(name) = component else {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "unsafe managed path component",
                ));
            };
            match open_target_dir_nofollow(&current, Path::new(name)) {
                Ok(next) => current = next,
                Err(error) if create && error.kind() == std::io::ErrorKind::NotFound => {
                    match current.create_dir(name) {
                        Ok(()) => {}
                        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                        Err(error) => return Err(error),
                    }
                    current = open_target_dir_nofollow(&current, Path::new(name))?;
                }
                Err(error) => return Err(error),
            }
        }
    }
    Ok((current, leaf))
}

fn open_target_dir_nofollow(parent: &Dir, name: &Path) -> Result<Dir, std::io::Error> {
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

fn open_target_file_nofollow(
    parent: &Dir,
    name: &Path,
) -> Result<cap_std::fs::File, std::io::Error> {
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
    let file = parent.open_with(name, &options)?;
    let metadata = file.metadata()?;
    if metadata_is_reparse_or_symlink(&metadata) || !metadata.is_file() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "managed path is not an ordinary file",
        ));
    }
    Ok(file)
}

fn remove_target_entry(parent: &Dir, leaf: &Path) -> Result<(), std::io::Error> {
    match parent.symlink_metadata(leaf) {
        Ok(metadata) if metadata_is_unsupported_reparse(&metadata) => Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "unsupported reparse point in managed target path",
        )),
        #[cfg(windows)]
        Ok(metadata)
            if metadata.file_type().is_symlink() && metadata_has_directory_attribute(&metadata) =>
        {
            parent.remove_dir(leaf)
        }
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

#[cfg(windows)]
fn metadata_is_unsupported_reparse(metadata: &cap_std::fs::Metadata) -> bool {
    metadata_is_reparse_or_symlink(metadata) && !metadata.file_type().is_symlink()
}

#[cfg(not(windows))]
fn metadata_is_unsupported_reparse(_metadata: &cap_std::fs::Metadata) -> bool {
    false
}

#[cfg(windows)]
fn metadata_has_directory_attribute(metadata: &cap_std::fs::Metadata) -> bool {
    use cap_std::fs::MetadataExt;
    const FILE_ATTRIBUTE_DIRECTORY: u32 = 0x0000_0010;
    metadata.file_attributes() & FILE_ATTRIBUTE_DIRECTORY != 0
}

fn inspect_symlink_target_kind(
    root: &Dir,
    link: &NormalizedManagedPath,
    target: &str,
) -> Result<SymlinkTargetKind, TargetFilesystemError> {
    let target = SafeSymlinkTarget::parse(link, target.to_owned())?;
    let resolved = target.resolved_for(link)?;
    let (parent, leaf) = open_target_parent_nofollow(root, &resolved, false)?;
    let metadata = parent.symlink_metadata(&leaf)?;
    if metadata_is_reparse_or_symlink(&metadata) {
        return Err(TargetFilesystemError::UnsupportedResource(
            resolved.to_string(),
        ));
    }
    if metadata.is_dir() {
        Ok(SymlinkTargetKind::Directory)
    } else if metadata.is_file() {
        Ok(SymlinkTargetKind::File)
    } else {
        Err(TargetFilesystemError::UnsupportedResource(
            resolved.to_string(),
        ))
    }
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

#[cfg(windows)]
fn std_metadata_is_reparse_or_symlink(metadata: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const FILE_ATTRIBUTE_REPARSE_POINT: u32 = 0x0000_0400;
    metadata.file_type().is_symlink()
        || metadata.file_attributes() & FILE_ATTRIBUTE_REPARSE_POINT != 0
}

#[cfg(not(windows))]
fn std_metadata_is_reparse_or_symlink(metadata: &std::fs::Metadata) -> bool {
    metadata.file_type().is_symlink()
}

#[cfg(unix)]
fn target_directory_mode(metadata: &cap_std::fs::Metadata) -> Option<u32> {
    use cap_std::fs::MetadataExt;
    Some(metadata.mode() & 0o7777)
}

#[cfg(not(unix))]
fn target_directory_mode(_metadata: &cap_std::fs::Metadata) -> Option<u32> {
    None
}

#[cfg(unix)]
fn set_target_directory_mode(directory: &Dir, mode: Option<u32>) -> Result<(), std::io::Error> {
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
fn set_target_directory_mode(_directory: &Dir, _mode: Option<u32>) -> Result<(), std::io::Error> {
    Ok(())
}

#[cfg(not(windows))]
fn create_target_symlink(
    parent: &Dir,
    target: &str,
    leaf: &Path,
    _target_kind: SymlinkTargetKind,
) -> Result<(), std::io::Error> {
    parent.symlink(target, leaf)
}

#[cfg(windows)]
fn create_target_symlink(
    parent: &Dir,
    target: &str,
    leaf: &Path,
    target_kind: SymlinkTargetKind,
) -> Result<(), std::io::Error> {
    match target_kind {
        SymlinkTargetKind::File => parent.symlink_file(target, leaf),
        SymlinkTargetKind::Directory => parent.symlink_dir(target, leaf),
    }
}
