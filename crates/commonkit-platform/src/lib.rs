//! Cross-platform local paths and operating-system boundaries.

use std::path::{Path, PathBuf};

use directories::ProjectDirs;
use thiserror::Error;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppPaths {
    pub config: PathBuf,
    pub state: PathBuf,
    pub cache: PathBuf,
    pub receipts: PathBuf,
    pub plans: PathBuf,
    pub backups: PathBuf,
    pub snapshots: PathBuf,
}

impl AppPaths {
    pub fn discover() -> Result<Self, PlatformError> {
        let project = ProjectDirs::from("com", "unsoldgroup", "CommonKit")
            .ok_or(PlatformError::HomeUnavailable)?;
        Self::from_roots(
            project.config_dir(),
            project.data_local_dir(),
            project.cache_dir(),
        )
    }

    pub fn from_roots(
        config: impl AsRef<Path>,
        data_local: impl AsRef<Path>,
        cache: impl AsRef<Path>,
    ) -> Result<Self, PlatformError> {
        let config = validate_root(config.as_ref())?;
        let state = validate_root(data_local.as_ref())?.join("state");
        let cache = validate_root(cache.as_ref())?;
        if config == state || config == cache || state == cache {
            return Err(PlatformError::OverlappingRoots);
        }
        Ok(Self {
            receipts: state.join("receipts"),
            plans: state.join("plans"),
            backups: state.join("backups"),
            snapshots: state.join("snapshots"),
            config,
            state,
            cache,
        })
    }

    pub fn create_private_roots(&self) -> Result<(), PlatformError> {
        for directory in [
            &self.config,
            &self.state,
            &self.cache,
            &self.receipts,
            &self.plans,
            &self.backups,
            &self.snapshots,
        ] {
            std::fs::create_dir_all(directory)?;
            set_private_directory_permissions(directory)?;
        }
        Ok(())
    }
}

fn validate_root(path: &Path) -> Result<PathBuf, PlatformError> {
    if !path.is_absolute() || path.parent().is_none() {
        return Err(PlatformError::UnsafeRoot(path.to_path_buf()));
    }
    Ok(path.to_path_buf())
}

#[cfg(unix)]
fn set_private_directory_permissions(path: &Path) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt;

    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_private_directory_permissions(_path: &Path) -> Result<(), std::io::Error> {
    Ok(())
}

#[derive(Debug, Error)]
pub enum PlatformError {
    #[error("the operating system did not provide a user application directory")]
    HomeUnavailable,
    #[error("application root must be absolute and cannot be a filesystem root: {0}")]
    UnsafeRoot(PathBuf),
    #[error("application config, state, and cache roots must be distinct")]
    OverlappingRoots,
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
