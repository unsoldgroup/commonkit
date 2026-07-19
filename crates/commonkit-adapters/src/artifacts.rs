use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use cap_std::fs::{Dir, OpenOptions};
use commonkit_contracts::Sha256Digest;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentSensitivity {
    Portable,
    LocalSensitive,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ContentReference {
    pub digest: Sha256Digest,
    pub bytes: u64,
    pub sensitivity: ContentSensitivity,
}

#[derive(Debug)]
pub struct ArtifactStore {
    directory: Dir,
}

impl ArtifactStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, ArtifactError> {
        let directory = open_or_create_directory_handle(root.as_ref())?;
        let metadata = directory.dir_metadata()?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(ArtifactError::InvalidRoot);
        }
        set_private_directory(&directory)?;
        Ok(Self { directory })
    }

    /// Opens a store for integrity validation without changing the filesystem.
    ///
    /// Unlike [`Self::open`], this never creates the root or repairs its
    /// permissions. Execution-time authority checks use this path so rejecting
    /// a stale or malformed plan is a strictly read-only operation.
    pub fn open_existing(root: impl AsRef<Path>) -> Result<Self, ArtifactError> {
        let directory = open_existing_directory_handle(root.as_ref())?;
        let metadata = directory.dir_metadata()?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(ArtifactError::InvalidRoot);
        }
        if !has_private_directory_permissions(&metadata) {
            return Err(ArtifactError::InvalidRoot);
        }
        Ok(Self { directory })
    }

    pub fn put(
        &self,
        bytes: &[u8],
        sensitivity: ContentSensitivity,
    ) -> Result<ContentReference, ArtifactError> {
        let reference = ContentReference {
            digest: digest_bytes(bytes)?,
            bytes: bytes
                .len()
                .try_into()
                .map_err(|_| ArtifactError::ArtifactTooLarge)?,
            sensitivity,
        };
        let path = self.relative_path_for(&reference.digest);
        let mut options = nofollow_options();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use cap_std::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match self.directory.open_with(&path, &options) {
            Ok(mut file) => {
                file.write_all(bytes)?;
                file.sync_all()?;
                sync_directory(&self.directory)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                self.load(&reference)?;
            }
            Err(error) => return Err(error.into()),
        }
        Ok(reference)
    }

    pub fn load(&self, reference: &ContentReference) -> Result<Vec<u8>, ArtifactError> {
        let bytes = self.load_unbounded(&reference.digest)?;
        if u64::try_from(bytes.len()).ok() != Some(reference.bytes)
            || digest_bytes(&bytes)? != reference.digest
        {
            return Err(ArtifactError::DigestMismatch);
        }
        Ok(bytes)
    }

    pub fn verify_digest(&self, digest: &Sha256Digest) -> Result<(), ArtifactError> {
        let bytes = self.load_unbounded(digest)?;
        if digest_bytes(&bytes)? == *digest {
            Ok(())
        } else {
            Err(ArtifactError::DigestMismatch)
        }
    }

    fn relative_path_for(&self, digest: &Sha256Digest) -> PathBuf {
        PathBuf::from(format!(
            "{}.blob",
            digest.as_str().trim_start_matches("sha256:")
        ))
    }

    fn load_unbounded(&self, digest: &Sha256Digest) -> Result<Vec<u8>, ArtifactError> {
        let mut file = self.open_artifact(digest)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        Ok(bytes)
    }

    fn open_artifact(&self, digest: &Sha256Digest) -> Result<cap_std::fs::File, ArtifactError> {
        let mut options = nofollow_options();
        options.read(true);
        let file = self
            .directory
            .open_with(self.relative_path_for(digest), &options)
            .map_err(map_unsafe_open)?;
        let metadata = file.metadata()?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(ArtifactError::UnsafeArtifactType);
        }
        Ok(file)
    }
}

fn open_or_create_directory_handle(path: &Path) -> Result<Dir, ArtifactError> {
    let absolute = absolute_path(path)?;
    let (mut current, components) = trusted_root_and_components(&absolute)?;
    if components.is_empty() {
        return Err(ArtifactError::InvalidRoot);
    }
    for name in components {
        let name = Path::new(&name);
        match open_directory_component(&current, name) {
            Ok(next) => current = next,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                match current.create_dir(name) {
                    Ok(()) => {}
                    Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error.into()),
                }
                let next = open_directory_component(&current, name)
                    .map_err(|_| ArtifactError::InvalidRoot)?;
                set_private_directory(&next)?;
                current = next;
            }
            Err(_) => return Err(ArtifactError::InvalidRoot),
        }
    }
    Ok(current)
}

fn open_existing_directory_handle(path: &Path) -> Result<Dir, ArtifactError> {
    let absolute = absolute_path(path)?;
    let (mut current, components) = trusted_root_and_components(&absolute)?;
    if components.is_empty() {
        return Err(ArtifactError::InvalidRoot);
    }
    for name in components {
        current = open_directory_component(&current, Path::new(&name))
            .map_err(|_| ArtifactError::InvalidRoot)?;
    }
    Ok(current)
}

fn absolute_path(path: &Path) -> Result<PathBuf, ArtifactError> {
    Ok(if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()?.join(path)
    })
}

#[cfg(unix)]
fn trusted_root_and_components(
    absolute: &Path,
) -> Result<(Dir, Vec<std::ffi::OsString>), ArtifactError> {
    use std::path::Component;

    let mut components = absolute.components();
    if !matches!(components.next(), Some(Component::RootDir)) {
        return Err(ArtifactError::InvalidRoot);
    }
    let mut names = components
        .map(|component| match component {
            Component::Normal(name) => Ok(name.to_os_string()),
            _ => Err(ArtifactError::InvalidRoot),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let root = Dir::open_ambient_dir(Path::new("/"), cap_std::ambient_authority())
        .map_err(|_| ArtifactError::InvalidRoot)?;
    // macOS exposes trusted root aliases such as `/var -> private/var` and
    // `/tmp -> private/tmp`. Resolve only this root-owned first component by
    // reading it through the retained `/` capability. Every resulting and
    // subsequent component is still opened individually with no-follow.
    if let Some(first) = names.first()
        && root
            .symlink_metadata(first)
            .is_ok_and(|metadata| metadata.file_type().is_symlink())
    {
        let target = root
            .read_link(first)
            .map_err(|_| ArtifactError::InvalidRoot)?;
        let mut target_names = Vec::new();
        for component in target.components() {
            match component {
                Component::RootDir => target_names.clear(),
                Component::Normal(name) => target_names.push(name.to_os_string()),
                _ => return Err(ArtifactError::InvalidRoot),
            }
        }
        if target_names.is_empty() {
            return Err(ArtifactError::InvalidRoot);
        }
        target_names.extend(names.into_iter().skip(1));
        names = target_names;
    }
    Ok((root, names))
}

#[cfg(windows)]
fn trusted_root_and_components(
    absolute: &Path,
) -> Result<(Dir, Vec<std::ffi::OsString>), ArtifactError> {
    use std::path::Component;

    let mut components = absolute.components();
    let prefix = match components.next() {
        Some(Component::Prefix(prefix)) => prefix.as_os_str().to_os_string(),
        _ => return Err(ArtifactError::InvalidRoot),
    };
    if !matches!(components.next(), Some(Component::RootDir)) {
        return Err(ArtifactError::InvalidRoot);
    }
    let mut volume_root = PathBuf::from(prefix);
    volume_root.push(std::path::MAIN_SEPARATOR.to_string());
    let names = components
        .map(|component| match component {
            Component::Normal(name) => Ok(name.to_os_string()),
            _ => Err(ArtifactError::InvalidRoot),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let root = Dir::open_ambient_dir(&volume_root, cap_std::ambient_authority())
        .map_err(|_| ArtifactError::InvalidRoot)?;
    Ok((root, names))
}

#[cfg(not(any(unix, windows)))]
fn trusted_root_and_components(
    absolute: &Path,
) -> Result<(Dir, Vec<std::ffi::OsString>), ArtifactError> {
    use std::path::Component;

    let mut components = absolute.components();
    if !matches!(components.next(), Some(Component::RootDir)) {
        return Err(ArtifactError::InvalidRoot);
    }
    let names = components
        .map(|component| match component {
            Component::Normal(name) => Ok(name.to_os_string()),
            _ => Err(ArtifactError::InvalidRoot),
        })
        .collect::<Result<Vec<_>, _>>()?;
    let root = Dir::open_ambient_dir(Path::new("/"), cap_std::ambient_authority())
        .map_err(|_| ArtifactError::InvalidRoot)?;
    Ok((root, names))
}

fn open_directory_component(parent: &Dir, name: &Path) -> Result<Dir, std::io::Error> {
    let mut options = nofollow_options();
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
            "artifact path component is not an ordinary directory",
        ));
    }
    Ok(Dir::from_std_file(file.into_std()))
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

fn nofollow_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    #[cfg(unix)]
    {
        use cap_std::fs::OpenOptionsExt;
        options.custom_flags(libc::O_NOFOLLOW);
    }
    #[cfg(windows)]
    nofollow_windows(&mut options);
    options
}

#[cfg(unix)]
fn has_private_directory_permissions(metadata: &cap_std::fs::Metadata) -> bool {
    use cap_std::fs::PermissionsExt;

    metadata.permissions().mode() & 0o777 == 0o700
}

#[cfg(not(unix))]
fn has_private_directory_permissions(_metadata: &cap_std::fs::Metadata) -> bool {
    true
}

fn digest_bytes(bytes: &[u8]) -> Result<Sha256Digest, ArtifactError> {
    let digest = Sha256::digest(bytes);
    Sha256Digest::parse(format!("sha256:{digest:x}")).map_err(ArtifactError::Contract)
}

#[cfg(unix)]
fn set_private_directory(directory: &Dir) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt;

    directory
        .try_clone()?
        .into_std_file()
        .set_permissions(fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_private_directory(_directory: &Dir) -> Result<(), std::io::Error> {
    Ok(())
}

#[cfg(unix)]
fn sync_directory(directory: &Dir) -> Result<(), std::io::Error> {
    directory.try_clone()?.into_std_file().sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_directory: &Dir) -> Result<(), std::io::Error> {
    Ok(())
}

#[cfg(windows)]
fn nofollow_windows(options: &mut OpenOptions) {
    use cap_std::fs::OpenOptionsExt;

    const FILE_FLAG_OPEN_REPARSE_POINT: u32 = 0x0020_0000;
    options.custom_flags(FILE_FLAG_OPEN_REPARSE_POINT);
}

fn map_unsafe_open(error: std::io::Error) -> ArtifactError {
    #[cfg(unix)]
    if error.raw_os_error() == Some(libc::ELOOP)
        || error.kind() == std::io::ErrorKind::PermissionDenied
    {
        return ArtifactError::UnsafeArtifactType;
    }
    ArtifactError::Io(error)
}

#[derive(Debug, Error)]
pub enum ArtifactError {
    #[error("artifact store root is invalid")]
    InvalidRoot,
    #[error("artifact is too large")]
    ArtifactTooLarge,
    #[error("artifact path is not an ordinary file")]
    UnsafeArtifactType,
    #[error("artifact content does not match its reference")]
    DigestMismatch,
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Contract(#[from] commonkit_contracts::ContractError),
}
