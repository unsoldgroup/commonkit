use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};

use cap_std::fs::Dir;

use super::provider::ProviderFailure;

#[derive(Clone, Copy)]
pub(crate) enum SymlinkPolicy {
    Reject,
    Preserve,
}

pub(crate) struct PrivateSnapshotRoot(PathBuf);

impl std::ops::Deref for PrivateSnapshotRoot {
    type Target = Path;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl Drop for PrivateSnapshotRoot {
    fn drop(&mut self) {
        remove_private_snapshot_root(&self.0);
    }
}

pub(crate) fn create_private_snapshot_root(
    root: &Path,
) -> Result<PrivateSnapshotRoot, ProviderFailure> {
    let snapshot = root.join("immutable-inputs");
    match fs::symlink_metadata(&snapshot) {
        Ok(metadata) if metadata.file_type().is_symlink() => {
            return Err(ProviderFailure::Materialize(
                "provider immutable-input snapshot path was replaced by a symlink".into(),
            ));
        }
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(&snapshot)
            .map_err(|error| ProviderFailure::Materialize(error.to_string()))?,
        Ok(_) => {
            return Err(ProviderFailure::Materialize(
                "provider immutable-input snapshot path has an unexpected type".into(),
            ));
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(ProviderFailure::Materialize(error.to_string())),
    }
    fs::create_dir(&snapshot).map_err(|error| ProviderFailure::Materialize(error.to_string()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&snapshot, fs::Permissions::from_mode(0o700))
            .map_err(|error| ProviderFailure::Materialize(error.to_string()))?;
    }
    Ok(PrivateSnapshotRoot(snapshot))
}

fn remove_private_snapshot_root(root: &Path) {
    if fs::symlink_metadata(root)
        .is_ok_and(|metadata| metadata.is_dir() && !metadata.file_type().is_symlink())
    {
        let _ = fs::remove_dir_all(root);
    }
}

pub(crate) fn copy_regular_file(
    source: &Path,
    destination: &Path,
    label: &str,
) -> Result<Vec<u8>, ProviderFailure> {
    let mut input = open_regular_file_no_follow(source).map_err(|error| {
        ProviderFailure::Materialize(format!("could not snapshot {label}: {error}"))
    })?;
    let metadata = input.metadata().map_err(|error| {
        ProviderFailure::Materialize(format!("could not snapshot {label}: {error}"))
    })?;
    if !metadata.is_file() {
        return Err(ProviderFailure::Materialize(format!(
            "could not snapshot {label}: source is not an ordinary file"
        )));
    }
    let mut bytes = Vec::new();
    input.read_to_end(&mut bytes).map_err(|error| {
        ProviderFailure::Materialize(format!("could not snapshot {label}: {error}"))
    })?;
    if let Some(parent) = destination.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| ProviderFailure::Materialize(error.to_string()))?;
    }
    fs::write(destination, &bytes)
        .map_err(|error| ProviderFailure::Materialize(error.to_string()))?;
    fs::set_permissions(destination, metadata.permissions())
        .map_err(|error| ProviderFailure::Materialize(error.to_string()))?;
    Ok(bytes)
}

pub(crate) fn copy_tree(
    source: &Path,
    destination: &Path,
    symlinks: SymlinkPolicy,
) -> Result<(), ProviderFailure> {
    let source = open_directory_no_follow(source)
        .map_err(|error| ProviderFailure::Materialize(error.to_string()))?;
    copy_tree_from_handle(&source, destination, symlinks)
}

fn copy_tree_from_handle(
    source: &Dir,
    destination: &Path,
    symlinks: SymlinkPolicy,
) -> Result<(), ProviderFailure> {
    fs::create_dir(destination).map_err(|error| ProviderFailure::Materialize(error.to_string()))?;
    let source_directory = source
        .try_clone()
        .map_err(|error| ProviderFailure::Materialize(error.to_string()))?
        .into_std_file();
    let source_permissions = source_directory
        .metadata()
        .map_err(|error| ProviderFailure::Materialize(error.to_string()))?
        .permissions();

    let mut entries = source
        .entries()
        .map_err(|error| ProviderFailure::Materialize(error.to_string()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ProviderFailure::Materialize(error.to_string()))?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let name = entry.file_name();
        let destination_path = destination.join(&name);
        let file_type = entry
            .file_type()
            .map_err(|error| ProviderFailure::Materialize(error.to_string()))?;
        if file_type.is_symlink() {
            if matches!(symlinks, SymlinkPolicy::Reject) {
                return Err(ProviderFailure::Materialize(format!(
                    "provider input contains unsupported symlink {}",
                    name.to_string_lossy()
                )));
            }
            let target = source
                .read_link(&name)
                .map_err(|error| ProviderFailure::Materialize(error.to_string()))?;
            preserve_symlink(&target, &destination_path)?;
        } else if file_type.is_dir() {
            let child = entry
                .open_dir()
                .map_err(|error| ProviderFailure::Materialize(error.to_string()))?;
            copy_tree_from_handle(&child, &destination_path, symlinks)?;
        } else if file_type.is_file() {
            let mut input = entry
                .open()
                .map_err(|error| ProviderFailure::Materialize(error.to_string()))?
                .into_std();
            let metadata = input
                .metadata()
                .map_err(|error| ProviderFailure::Materialize(error.to_string()))?;
            if !metadata.is_file() {
                return Err(ProviderFailure::Materialize(
                    "provider input changed type while being snapshotted".into(),
                ));
            }
            let mut bytes = Vec::new();
            input
                .read_to_end(&mut bytes)
                .map_err(|error| ProviderFailure::Materialize(error.to_string()))?;
            fs::write(&destination_path, bytes)
                .map_err(|error| ProviderFailure::Materialize(error.to_string()))?;
            fs::set_permissions(&destination_path, metadata.permissions())
                .map_err(|error| ProviderFailure::Materialize(error.to_string()))?;
        } else {
            return Err(ProviderFailure::Materialize(format!(
                "provider input contains unsupported filesystem object {}",
                name.to_string_lossy()
            )));
        }
    }
    fs::set_permissions(destination, source_permissions)
        .map_err(|error| ProviderFailure::Materialize(error.to_string()))?;
    Ok(())
}

#[cfg(unix)]
fn preserve_symlink(target: &Path, destination: &Path) -> Result<(), ProviderFailure> {
    std::os::unix::fs::symlink(target, destination)
        .map_err(|error| ProviderFailure::Materialize(error.to_string()))
}

#[cfg(windows)]
fn preserve_symlink(target: &Path, destination: &Path) -> Result<(), ProviderFailure> {
    use std::os::windows::fs::{symlink_dir, symlink_file};
    if target.has_root() {
        return Err(ProviderFailure::Materialize(
            "provider input contains an absolute symlink target".into(),
        ));
    }
    // Windows requires choosing a link type at creation. Source-tree links are
    // overwhelmingly file declarations; a directory target is retried when needed.
    symlink_file(target, destination)
        .or_else(|_| symlink_dir(target, destination))
        .map_err(|error| ProviderFailure::Materialize(error.to_string()))
}

#[cfg(unix)]
fn open_regular_file_no_follow(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(windows)]
fn open_regular_file_no_follow(path: &Path) -> std::io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::FILE_FLAG_OPEN_REPARSE_POINT;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}

fn open_directory_no_follow(path: &Path) -> std::io::Result<Dir> {
    let metadata = fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() || !metadata.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "provider snapshot root is not an ordinary directory",
        ));
    }
    let directory = Dir::from_std_file(open_directory_handle_no_follow(path)?);
    if !directory.dir_metadata()?.is_dir() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "provider snapshot root changed type while being opened",
        ));
    }
    Ok(directory)
}

#[cfg(unix)]
fn open_directory_handle_no_follow(path: &Path) -> std::io::Result<fs::File> {
    use std::os::unix::fs::OpenOptionsExt;
    fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
        .open(path)
}

#[cfg(windows)]
fn open_directory_handle_no_follow(path: &Path) -> std::io::Result<fs::File> {
    use std::os::windows::fs::OpenOptionsExt;
    use windows_sys::Win32::Storage::FileSystem::{
        FILE_FLAG_BACKUP_SEMANTICS, FILE_FLAG_OPEN_REPARSE_POINT, FILE_SHARE_READ, FILE_SHARE_WRITE,
    };
    fs::OpenOptions::new()
        .read(true)
        .share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS | FILE_FLAG_OPEN_REPARSE_POINT)
        .open(path)
}
