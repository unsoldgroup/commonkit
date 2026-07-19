//! Cross-platform local paths and operating-system boundaries.

use std::ffi::{OsStr, OsString};
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
        if let Some(paths) = configured_app_paths(|name| std::env::var_os(name))? {
            return Ok(paths);
        }
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

fn configured_app_paths(
    get: impl Fn(&OsStr) -> Option<OsString>,
) -> Result<Option<AppPaths>, PlatformError> {
    let config = get(OsStr::new("XDG_CONFIG_HOME"));
    let data = get(OsStr::new("XDG_DATA_HOME"));
    let cache = get(OsStr::new("XDG_CACHE_HOME"));
    match (config, data, cache) {
        (None, None, None) => Ok(None),
        (Some(config), Some(data), Some(cache)) => AppPaths::from_roots(
            PathBuf::from(config),
            PathBuf::from(data),
            PathBuf::from(cache),
        )
        .map(Some),
        _ => Err(PlatformError::IncompleteRootOverride),
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
    let arguments = windows_private_acl_args(&user)?;
    let status = std::process::Command::new("icacls.exe")
        .arg(path)
        .args(arguments)
        .status()?;
    if !status.success() {
        return Err(PlatformError::AclFailed(path.to_path_buf()));
    }
    Ok(())
}

pub fn windows_private_acl_args(user: &str) -> Result<Vec<String>, PlatformError> {
    if user.is_empty() || user.contains(['\r', '\n', ':']) {
        return Err(PlatformError::IdentityUnavailable);
    }
    Ok(vec![
        "/inheritance:r".into(),
        "/remove:g".into(),
        "*S-1-1-0".into(),
        "*S-1-5-11".into(),
        "*S-1-5-32-545".into(),
        "*S-1-15-2-1".into(),
        "/grant:r".into(),
        format!("{user}:(F)"),
    ])
}

#[cfg(windows)]
fn verify_private_permissions(
    path: &Path,
    _metadata: &std::fs::Metadata,
    _kind: PrivatePathKind,
) -> Result<(), PlatformError> {
    let user = std::env::var("USERNAME").map_err(|_| PlatformError::IdentityUnavailable)?;
    let output = std::process::Command::new("icacls.exe")
        .arg(path)
        .output()?;
    if !output.status.success()
        || !windows_acl_listing_is_private(&String::from_utf8_lossy(&output.stdout), &user)
    {
        return Err(PlatformError::AclFailed(path.to_path_buf()));
    }
    Ok(())
}

/// Validates the security-relevant subset of `icacls <path>` output. `/verify`
/// only checks canonical ACL structure; it does not reject broad or inherited grants.
pub fn windows_acl_listing_is_private(listing: &str, user: &str) -> bool {
    if user.is_empty() || user.contains(['\r', '\n', ':']) || listing.is_empty() {
        return false;
    }
    let normalized = listing.to_ascii_lowercase();
    let user = user.to_ascii_lowercase();
    let broad = [
        "everyone:",
        "builtin\\users:",
        "authenticated users:",
        "all application packages:",
        "todos:",
        "utilisateurs authentifiés:",
    ];
    normalized.contains(&user)
        && normalized.contains("(f)")
        && !normalized.contains("(i)")
        && !broad.iter().any(|identity| normalized.contains(identity))
}

#[derive(Debug, Error)]
pub enum PlatformError {
    #[error("the operating system did not provide a user application directory")]
    HomeUnavailable,
    #[error("application root must be absolute and cannot be a filesystem root: {0}")]
    UnsafeRoot(PathBuf),
    #[error("application config, state, and cache roots must be distinct")]
    OverlappingRoots,
    #[error(
        "XDG_CONFIG_HOME, XDG_DATA_HOME, and XDG_CACHE_HOME must be set together to override CommonKit application roots"
    )]
    IncompleteRootOverride,
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::{OsStr, OsString};

    #[cfg(not(windows))]
    const CONFIG_ROOT: &str = "/isolated/config";
    #[cfg(windows)]
    const CONFIG_ROOT: &str = r"C:\isolated\config";
    #[cfg(not(windows))]
    const DATA_ROOT: &str = "/isolated/data";
    #[cfg(windows)]
    const DATA_ROOT: &str = r"C:\isolated\data";
    #[cfg(not(windows))]
    const CACHE_ROOT: &str = "/isolated/cache";
    #[cfg(windows)]
    const CACHE_ROOT: &str = r"C:\isolated\cache";

    fn roots(values: &[(&str, &str)]) -> Result<Option<AppPaths>, PlatformError> {
        configured_app_paths(|name| {
            values
                .iter()
                .find(|(key, _)| OsStr::new(key) == name)
                .map(|(_, value)| OsString::from(value))
        })
    }

    #[test]
    fn complete_explicit_roots_are_used_without_platform_directory_discovery() {
        let paths = roots(&[
            ("XDG_CONFIG_HOME", CONFIG_ROOT),
            ("XDG_DATA_HOME", DATA_ROOT),
            ("XDG_CACHE_HOME", CACHE_ROOT),
        ])
        .unwrap()
        .unwrap();

        assert_eq!(paths.config, Path::new(CONFIG_ROOT));
        assert_eq!(paths.state, Path::new(DATA_ROOT).join("state"));
        assert_eq!(paths.cache, Path::new(CACHE_ROOT));
    }

    #[test]
    fn partial_or_unsafe_explicit_roots_fail_closed() {
        assert!(matches!(
            roots(&[("XDG_CONFIG_HOME", CONFIG_ROOT)]),
            Err(PlatformError::IncompleteRootOverride)
        ));
        assert!(matches!(
            roots(&[
                ("XDG_CONFIG_HOME", CONFIG_ROOT),
                ("XDG_DATA_HOME", "relative-data"),
                ("XDG_CACHE_HOME", CACHE_ROOT),
            ]),
            Err(PlatformError::UnsafeRoot(_))
        ));
    }

    #[test]
    fn absent_explicit_roots_preserve_platform_directory_discovery() {
        assert_eq!(roots(&[]).unwrap(), None);
    }
}
