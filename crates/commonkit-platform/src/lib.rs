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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPlatform {
    MacOs,
    Linux,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecurityCapability {
    PosixModes,
    WindowsAcl,
    Keychain,
    SecretService,
    CredentialManager,
    ProcessSandbox,
}

impl HostPlatform {
    pub fn current() -> Result<Self, PlatformError> {
        match std::env::consts::OS {
            "macos" => Ok(Self::MacOs),
            "linux" => Ok(Self::Linux),
            "windows" => Ok(Self::Windows),
            other => Err(PlatformError::UnsupportedPlatform(other.to_owned())),
        }
    }

    pub fn security_capabilities(self) -> &'static [SecurityCapability] {
        match self {
            Self::MacOs => &[
                SecurityCapability::PosixModes,
                SecurityCapability::Keychain,
                SecurityCapability::ProcessSandbox,
            ],
            Self::Linux => &[
                SecurityCapability::PosixModes,
                SecurityCapability::SecretService,
                SecurityCapability::ProcessSandbox,
            ],
            Self::Windows => &[
                SecurityCapability::WindowsAcl,
                SecurityCapability::CredentialManager,
                SecurityCapability::ProcessSandbox,
            ],
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PrivatePathKind {
    Directory,
    File,
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
            ensure_private_path(directory, PrivatePathKind::Directory)?;
        }
        Ok(())
    }
}

pub fn ensure_private_path(path: &Path, kind: PrivatePathKind) -> Result<(), PlatformError> {
    if let Ok(metadata) = std::fs::symlink_metadata(path) {
        if metadata.file_type().is_symlink() {
            return Err(PlatformError::SymbolicLink(path.to_path_buf()));
        }
        let type_matches = match kind {
            PrivatePathKind::Directory => metadata.is_dir(),
            PrivatePathKind::File => metadata.is_file(),
        };
        if !type_matches {
            return Err(PlatformError::WrongPathType(path.to_path_buf()));
        }
    } else {
        match kind {
            PrivatePathKind::Directory => std::fs::create_dir_all(path)?,
            PrivatePathKind::File => {
                if let Some(parent) = path.parent() {
                    ensure_private_path(parent, PrivatePathKind::Directory)?;
                }
                std::fs::OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .open(path)?;
            }
        }
    }
    set_private_permissions(path, kind)?;
    verify_private_path(path, kind)
}

pub fn verify_private_path(path: &Path, kind: PrivatePathKind) -> Result<(), PlatformError> {
    let metadata = std::fs::symlink_metadata(path)?;
    if metadata.file_type().is_symlink() {
        return Err(PlatformError::SymbolicLink(path.to_path_buf()));
    }
    let type_matches = match kind {
        PrivatePathKind::Directory => metadata.is_dir(),
        PrivatePathKind::File => metadata.is_file(),
    };
    if !type_matches {
        return Err(PlatformError::WrongPathType(path.to_path_buf()));
    }
    verify_private_permissions(path, &metadata, kind)
}

fn validate_root(path: &Path) -> Result<PathBuf, PlatformError> {
    if !path.is_absolute() || path.parent().is_none() {
        return Err(PlatformError::UnsafeRoot(path.to_path_buf()));
    }
    Ok(path.to_path_buf())
}

#[cfg(unix)]
fn set_private_permissions(path: &Path, kind: PrivatePathKind) -> Result<(), PlatformError> {
    use std::os::unix::fs::PermissionsExt;
    let mode = match kind {
        PrivatePathKind::Directory => 0o700,
        PrivatePathKind::File => 0o600,
    };
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(mode))?;
    Ok(())
}

#[cfg(unix)]
fn verify_private_permissions(
    path: &Path,
    metadata: &std::fs::Metadata,
    kind: PrivatePathKind,
) -> Result<(), PlatformError> {
    use std::os::unix::fs::PermissionsExt;
    let expected = match kind {
        PrivatePathKind::Directory => 0o700,
        PrivatePathKind::File => 0o600,
    };
    if metadata.permissions().mode() & 0o777 != expected {
        return Err(PlatformError::InsecurePermissions(path.to_path_buf()));
    }
    Ok(())
}

#[cfg(windows)]
fn set_private_permissions(path: &Path, _kind: PrivatePathKind) -> Result<(), PlatformError> {
    let user = std::env::var("USERNAME").map_err(|_| PlatformError::IdentityUnavailable)?;
    if user.is_empty() || user.contains(['\r', '\n', ':']) {
        return Err(PlatformError::IdentityUnavailable);
    }
    let grant = format!("{user}:(F)");
    let status = std::process::Command::new("icacls.exe")
        .arg(path)
        .args(["/inheritance:r", "/grant:r", &grant])
        .status()?;
    if !status.success() {
        return Err(PlatformError::AclFailed(path.to_path_buf()));
    }
    Ok(())
}

#[cfg(windows)]
fn verify_private_permissions(
    path: &Path,
    _metadata: &std::fs::Metadata,
    _kind: PrivatePathKind,
) -> Result<(), PlatformError> {
    let status = std::process::Command::new("icacls.exe")
        .arg(path)
        .arg("/verify")
        .status()?;
    if !status.success() {
        return Err(PlatformError::AclFailed(path.to_path_buf()));
    }
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
    #[error("unsupported operating system: {0}")]
    UnsupportedPlatform(String),
    #[error("private path cannot be a symbolic link: {0}")]
    SymbolicLink(PathBuf),
    #[error("private path has the wrong resource type: {0}")]
    WrongPathType(PathBuf),
    #[error("private path permissions are not restrictive: {0}")]
    InsecurePermissions(PathBuf),
    #[error("the current platform identity is unavailable")]
    IdentityUnavailable,
    #[error("failed to apply or verify a private Windows ACL: {0}")]
    AclFailed(PathBuf),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}
