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
        _ => Ok(None),
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
    windows_acl::set_current_user_only(path)
}

#[cfg(windows)]
fn verify_private_permissions(
    path: &Path,
    _metadata: &std::fs::Metadata,
    _kind: PrivatePathKind,
) -> Result<(), PlatformError> {
    windows_acl::verify_current_user_only(path)
}

#[cfg(windows)]
mod windows_acl {
    use super::PlatformError;
    use std::ffi::c_void;
    use std::mem::{size_of, zeroed};
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, LocalFree};
    use windows_sys::Win32::Security::Authorization::{
        BuildTrusteeWithSidW, EXPLICIT_ACCESS_W, SE_FILE_OBJECT, SET_ACCESS, SetEntriesInAclW,
        SetNamedSecurityInfoW,
    };
    use windows_sys::Win32::Security::{
        ACCESS_ALLOWED_ACE, ACL, ACL_SIZE_INFORMATION, AclSizeInformation,
        DACL_SECURITY_INFORMATION, EqualSid, GetAce, GetAclInformation, GetLengthSid,
        GetSecurityDescriptorControl, GetTokenInformation, INHERITED_ACE, NO_INHERITANCE,
        PROTECTED_DACL_SECURITY_INFORMATION, SE_DACL_PROTECTED, TOKEN_QUERY, TOKEN_USER, TokenUser,
    };
    use windows_sys::Win32::Storage::FileSystem::FILE_ALL_ACCESS;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, OpenProcessToken};

    struct Token(HANDLE);
    impl Drop for Token {
        fn drop(&mut self) {
            unsafe { CloseHandle(self.0) };
        }
    }

    fn current_user_sid() -> Result<Vec<u8>, PlatformError> {
        let mut handle = null_mut();
        if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut handle) } == 0 {
            return Err(PlatformError::IdentityUnavailable);
        }
        let token = Token(handle);
        let mut length = 0;
        unsafe { GetTokenInformation(token.0, TokenUser, null_mut(), 0, &mut length) };
        if length == 0 {
            return Err(PlatformError::IdentityUnavailable);
        }
        let mut token_user = vec![0_u8; length as usize];
        if unsafe {
            GetTokenInformation(
                token.0,
                TokenUser,
                token_user.as_mut_ptr().cast(),
                length,
                &mut length,
            )
        } == 0
        {
            return Err(PlatformError::IdentityUnavailable);
        }
        let sid = unsafe { (*(token_user.as_ptr().cast::<TOKEN_USER>())).User.Sid };
        let sid_length = unsafe { GetLengthSid(sid) } as usize;
        if sid.is_null() || sid_length == 0 {
            return Err(PlatformError::IdentityUnavailable);
        }
        Ok(unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), sid_length) }.to_vec())
    }

    pub(super) fn set_current_user_only(path: &Path) -> Result<(), PlatformError> {
        let mut sid = current_user_sid()?;
        let mut trustee = unsafe { zeroed() };
        unsafe { BuildTrusteeWithSidW(&mut trustee, sid.as_mut_ptr().cast()) };
        let entry = EXPLICIT_ACCESS_W {
            grfAccessPermissions: FILE_ALL_ACCESS,
            grfAccessMode: SET_ACCESS,
            grfInheritance: NO_INHERITANCE,
            Trustee: trustee,
        };
        let mut dacl: *mut ACL = null_mut();
        let result = unsafe { SetEntriesInAclW(1, &entry, null(), &mut dacl) };
        if result != 0 || dacl.is_null() {
            if !dacl.is_null() {
                unsafe { LocalFree(dacl.cast()) };
            }
            return Err(PlatformError::AclFailed(path.to_path_buf()));
        }
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let result = unsafe {
            SetNamedSecurityInfoW(
                wide.as_mut_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                dacl,
                null_mut(),
            )
        };
        unsafe { LocalFree(dacl.cast()) };
        if result != 0 {
            return Err(PlatformError::AclFailed(path.to_path_buf()));
        }
        Ok(())
    }

    pub(super) fn verify_current_user_only(path: &Path) -> Result<(), PlatformError> {
        use windows_sys::Win32::Security::Authorization::GetNamedSecurityInfoW;

        let mut sid = current_user_sid()?;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain([0]).collect();
        let mut dacl: *mut ACL = null_mut();
        let mut descriptor: *mut c_void = null_mut();
        let result = unsafe {
            GetNamedSecurityInfoW(
                wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                null_mut(),
                null_mut(),
                &mut dacl,
                null_mut(),
                &mut descriptor,
            )
        };
        if result != 0 || dacl.is_null() {
            if !descriptor.is_null() {
                unsafe { LocalFree(descriptor) };
            }
            return Err(PlatformError::AclFailed(path.to_path_buf()));
        }
        let mut control = 0;
        let mut revision = 0;
        let protected =
            unsafe { GetSecurityDescriptorControl(descriptor, &mut control, &mut revision) } != 0
                && control & SE_DACL_PROTECTED != 0;
        let mut info: ACL_SIZE_INFORMATION = unsafe { zeroed() };
        let valid = protected
            && unsafe {
                GetAclInformation(
                    dacl,
                    (&mut info as *mut ACL_SIZE_INFORMATION).cast(),
                    size_of::<ACL_SIZE_INFORMATION>() as u32,
                    AclSizeInformation,
                )
            } != 0
            && info.AceCount == 1;
        let mut ace: *mut c_void = null_mut();
        let valid = valid && unsafe { GetAce(dacl, 0, &mut ace) } != 0 && !ace.is_null();
        let valid = valid
            && unsafe {
                let allowed = &*ace.cast::<ACCESS_ALLOWED_ACE>();
                allowed.Header.AceType == 0
                    && u32::from(allowed.Header.AceFlags) & INHERITED_ACE == 0
                    && allowed.Mask == FILE_ALL_ACCESS
                    && EqualSid(
                        std::ptr::addr_of!(allowed.SidStart).cast_mut().cast(),
                        sid.as_mut_ptr().cast(),
                    ) != 0
            };
        unsafe { LocalFree(descriptor) };
        if !valid {
            return Err(PlatformError::AclFailed(path.to_path_buf()));
        }
        Ok(())
    }
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
    fn partial_standard_xdg_roots_preserve_platform_directory_discovery() {
        assert_eq!(roots(&[("XDG_CONFIG_HOME", CONFIG_ROOT)]).unwrap(), None);
        assert_eq!(
            roots(&[
                ("XDG_CONFIG_HOME", CONFIG_ROOT),
                ("XDG_CACHE_HOME", CACHE_ROOT),
            ])
            .unwrap(),
            None
        );
    }

    #[test]
    fn unsafe_complete_explicit_roots_fail_closed() {
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
