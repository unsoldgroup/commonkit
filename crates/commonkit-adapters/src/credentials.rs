use std::collections::BTreeMap;
use std::fmt;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use cap_std::ambient_authority;
use cap_std::fs::{Dir, OpenOptions};
#[cfg(unix)]
use cap_std::fs::{OpenOptionsExt, Permissions, PermissionsExt};

use serde::{Deserialize, Deserializer, Serialize};
use thiserror::Error;

/// A portable pointer to credential material. The pointed-to value is never part of this type.
#[derive(Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct CredentialReference(String);

impl CredentialReference {
    pub fn parse(value: impl Into<String>) -> Result<Self, CredentialReferenceError> {
        let value = value.into();
        if value.len() > 2048 || value.chars().any(char::is_whitespace) {
            return Err(CredentialReferenceError::InvalidReference);
        }
        let (scheme, opaque) = value
            .split_once("://")
            .ok_or(CredentialReferenceError::UnsupportedScheme)?;
        if opaque.is_empty() || opaque.contains(['?', '#', '\0']) {
            return Err(CredentialReferenceError::InvalidReference);
        }
        match scheme {
            "env" if valid_env_name(opaque) => {}
            "file" if valid_absolute_file_path(opaque) => {}
            "keychain" if valid_keychain_key(opaque) => {}
            "bws" if valid_opaque_key(opaque) => {}
            "env" | "file" | "keychain" | "bws" => {
                return Err(CredentialReferenceError::InvalidReference);
            }
            _ => return Err(CredentialReferenceError::UnsupportedScheme),
        }
        Ok(Self(value))
    }

    /// Builds a canonical credential reference for an absolute local file path.
    ///
    /// This is the only supported conversion from a platform path to the portable
    /// reference syntax; callers must not interpolate `Path::display()` into a URI.
    pub fn from_file_path(path: impl AsRef<Path>) -> Result<Self, CredentialReferenceError> {
        let path = path.as_ref();
        if !path.is_absolute() {
            return Err(CredentialReferenceError::InvalidReference);
        }
        let path = path
            .to_str()
            .ok_or(CredentialReferenceError::InvalidReference)?;

        #[cfg(windows)]
        let reference = {
            let normalized = path.replace('\\', "/");
            let bytes = normalized.as_bytes();
            if !matches!(bytes, [drive, b':', b'/', ..] if drive.is_ascii_alphabetic()) {
                return Err(CredentialReferenceError::InvalidReference);
            }
            format!("file:///{normalized}")
        };
        #[cfg(not(windows))]
        let reference = format!("file://{path}");

        Self::parse(reference)
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub fn scheme(&self) -> &str {
        self.0.split_once("://").expect("validated reference").0
    }

    pub fn opaque(&self) -> &str {
        self.0.split_once("://").expect("validated reference").1
    }
}

impl fmt::Debug for CredentialReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("CredentialReference(<redacted>)")
    }
}

impl fmt::Display for CredentialReference {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<credential-reference>")
    }
}

impl<'de> Deserialize<'de> for CredentialReference {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

fn valid_env_name(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|byte| byte == b'_' || byte.is_ascii_uppercase())
        && bytes.all(|byte| byte == b'_' || byte.is_ascii_uppercase() || byte.is_ascii_digit())
}

fn valid_absolute_file_path(value: &str) -> bool {
    value.starts_with('/')
        && value
            .split('/')
            .all(|component| component != "." && component != "..")
}

fn valid_keychain_key(value: &str) -> bool {
    let mut segments = value.split('/');
    matches!((segments.next(), segments.next(), segments.next()), (Some(service), Some(account), None) if valid_opaque_key(service) && valid_opaque_key(account))
}

fn valid_opaque_key(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
        && !value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CredentialReferenceError {
    #[error("unsupported credential reference scheme")]
    UnsupportedScheme,
    #[error("invalid credential reference")]
    InvalidReference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CredentialReadiness {
    Ready,
    Missing,
    Unavailable,
}

/// Read-only preflight. Implementations must only establish availability, never fetch values.
pub trait CredentialReadinessInspector {
    fn inspect(&self, reference: &CredentialReference) -> CredentialReadiness;
}

/// Side-effect-free local probes. Keychain and BWS remain unavailable until a dedicated
/// provider-specific probe is configured; inspection never invokes either provider.
#[derive(Debug, Default, Clone, Copy)]
pub struct LocalCredentialReadinessInspector;

impl CredentialReadinessInspector for LocalCredentialReadinessInspector {
    fn inspect(&self, reference: &CredentialReference) -> CredentialReadiness {
        match reference.scheme() {
            "env" => {
                if std::env::var_os(reference.opaque()).is_some() {
                    CredentialReadiness::Ready
                } else {
                    CredentialReadiness::Missing
                }
            }
            "file" => match std::fs::symlink_metadata(local_file_path(reference)) {
                Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
                    CredentialReadiness::Ready
                }
                Ok(_) => CredentialReadiness::Unavailable,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    CredentialReadiness::Missing
                }
                Err(_) => CredentialReadiness::Unavailable,
            },
            "keychain" | "bws" => CredentialReadiness::Unavailable,
            _ => CredentialReadiness::Unavailable,
        }
    }
}

fn local_file_path(reference: &CredentialReference) -> &Path {
    let opaque = reference.opaque();
    #[cfg(windows)]
    {
        let bytes = opaque.as_bytes();
        if matches!(bytes, [b'/', drive, b':', b'/', ..] if drive.is_ascii_alphabetic()) {
            return Path::new(&opaque[1..]);
        }
    }
    Path::new(opaque)
}

/// Secret bytes have no serialization or cloning surface and redact all formatting.
pub struct SecretValue(Vec<u8>);

impl SecretValue {
    pub fn new(bytes: Vec<u8>) -> Result<Self, CredentialResolveError> {
        if bytes.is_empty() {
            return Err(CredentialResolveError::EmptyValue);
        }
        Ok(Self(bytes))
    }

    /// This is intentionally named to make the sole permitted exposure point explicit.
    pub fn expose_for_apply(&self) -> &[u8] {
        &self.0
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        self.0.fill(0);
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretValue(<redacted>)")
    }
}

impl fmt::Display for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("<redacted>")
    }
}

pub trait CredentialResolver {
    fn resolve(
        &mut self,
        reference: &CredentialReference,
    ) -> Result<SecretValue, CredentialResolveError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlatformKeychain {
    MacOs,
    LinuxSecretService,
    WindowsCredentialManager,
}

impl PlatformKeychain {
    pub fn current() -> Result<Self, CredentialResolveError> {
        match std::env::consts::OS {
            "macos" => Ok(Self::MacOs),
            "linux" => Ok(Self::LinuxSecretService),
            "windows" => Ok(Self::WindowsCredentialManager),
            _ => Err(CredentialResolveError::UnsupportedReference),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("platform credential command failed")]
pub struct PlatformSecretCommandError;

pub trait PlatformSecretCommandRunner {
    fn run(
        &mut self,
        executable: &str,
        args: &[String],
    ) -> Result<Vec<u8>, PlatformSecretCommandError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ProcessPlatformSecretCommandRunner;

impl PlatformSecretCommandRunner for ProcessPlatformSecretCommandRunner {
    fn run(
        &mut self,
        executable: &str,
        args: &[String],
    ) -> Result<Vec<u8>, PlatformSecretCommandError> {
        let mut command = Command::new(executable);
        command
            .args(args)
            .env_clear()
            .stdin(Stdio::null())
            .stderr(Stdio::null());
        for name in ["DBUS_SESSION_BUS_ADDRESS", "XDG_RUNTIME_DIR"] {
            if let Some(value) = std::env::var_os(name) {
                command.env(name, value);
            }
        }
        let output = command.output().map_err(|_| PlatformSecretCommandError)?;
        if !output.status.success() || output.stdout.len() > 1024 * 1024 {
            return Err(PlatformSecretCommandError);
        }
        Ok(output.stdout)
    }
}

pub struct PlatformKeychainCredentialResolver<R> {
    platform: PlatformKeychain,
    runner: R,
}

impl<R> PlatformKeychainCredentialResolver<R> {
    pub fn new(platform: PlatformKeychain, runner: R) -> Self {
        Self { platform, runner }
    }

    pub fn runner(&self) -> &R {
        &self.runner
    }
}

impl<R: PlatformSecretCommandRunner> CredentialResolver for PlatformKeychainCredentialResolver<R> {
    fn resolve(
        &mut self,
        reference: &CredentialReference,
    ) -> Result<SecretValue, CredentialResolveError> {
        if reference.scheme() != "keychain" {
            return Err(CredentialResolveError::UnsupportedReference);
        }
        let (service, account) = reference
            .opaque()
            .split_once('/')
            .ok_or(CredentialResolveError::UnsupportedReference)?;
        let (executable, args) = match self.platform {
            PlatformKeychain::MacOs => (
                "/usr/bin/security",
                vec![
                    "find-generic-password".into(),
                    "-s".into(),
                    service.into(),
                    "-a".into(),
                    account.into(),
                    "-w".into(),
                ],
            ),
            PlatformKeychain::LinuxSecretService => (
                "/usr/bin/secret-tool",
                vec![
                    "lookup".into(),
                    "service".into(),
                    service.into(),
                    "account".into(),
                    account.into(),
                ],
            ),
            PlatformKeychain::WindowsCredentialManager => {
                #[cfg(windows)]
                {
                    let mut native = WindowsCredentialManagerResolver::default();
                    return native.resolve(reference);
                }
                #[cfg(not(windows))]
                {
                    return Err(CredentialResolveError::UnsupportedReference);
                }
            }
        };
        let mut bytes = self
            .runner
            .run(executable, &args)
            .map_err(|_| CredentialResolveError::ProviderFailed)?;
        while matches!(bytes.last(), Some(b'\n' | b'\r')) {
            bytes.pop();
        }
        SecretValue::new(bytes)
    }
}

pub trait WindowsCredentialReader {
    fn read_generic(&mut self, target: &str) -> Result<Vec<u8>, CredentialResolveError>;
}

#[derive(Debug, Default, Clone, Copy)]
pub struct NativeWindowsCredentialReader;

#[cfg(windows)]
impl WindowsCredentialReader for NativeWindowsCredentialReader {
    fn read_generic(&mut self, target: &str) -> Result<Vec<u8>, CredentialResolveError> {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Security::Credentials::{
            CRED_TYPE_GENERIC, CREDENTIALW, CredFree, CredReadW,
        };

        let wide = std::ffi::OsStr::new(target)
            .encode_wide()
            .chain(Some(0))
            .collect::<Vec<_>>();
        let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
        let success = unsafe { CredReadW(wide.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) };
        if success == 0 || credential.is_null() {
            return Err(CredentialResolveError::Unavailable);
        }
        struct OwnedCredential(*mut CREDENTIALW);
        impl Drop for OwnedCredential {
            fn drop(&mut self) {
                unsafe { CredFree(self.0.cast()) }
            }
        }
        let credential = OwnedCredential(credential);
        let native = unsafe { &*credential.0 };
        if native.CredentialBlobSize == 0 || native.CredentialBlob.is_null() {
            return Err(CredentialResolveError::EmptyValue);
        }
        Ok(unsafe {
            std::slice::from_raw_parts(native.CredentialBlob, native.CredentialBlobSize as usize)
        }
        .to_vec())
    }
}

#[cfg(not(windows))]
impl WindowsCredentialReader for NativeWindowsCredentialReader {
    fn read_generic(&mut self, _target: &str) -> Result<Vec<u8>, CredentialResolveError> {
        Err(CredentialResolveError::UnsupportedReference)
    }
}

pub struct WindowsCredentialManagerResolver<R = NativeWindowsCredentialReader> {
    reader: R,
}

impl<R> WindowsCredentialManagerResolver<R> {
    pub fn new(reader: R) -> Self {
        Self { reader }
    }
    pub fn reader(&self) -> &R {
        &self.reader
    }
}

impl Default for WindowsCredentialManagerResolver<NativeWindowsCredentialReader> {
    fn default() -> Self {
        Self::new(NativeWindowsCredentialReader)
    }
}

impl<R: WindowsCredentialReader> CredentialResolver for WindowsCredentialManagerResolver<R> {
    fn resolve(
        &mut self,
        reference: &CredentialReference,
    ) -> Result<SecretValue, CredentialResolveError> {
        if reference.scheme() != "keychain" {
            return Err(CredentialResolveError::UnsupportedReference);
        }
        let (service, account) = reference
            .opaque()
            .split_once('/')
            .ok_or(CredentialResolveError::UnsupportedReference)?;
        SecretValue::new(
            self.reader
                .read_generic(&format!("commonkit/{service}/{account}"))?,
        )
    }
}

pub struct FakeCredentialResolver {
    values: BTreeMap<CredentialReference, Vec<u8>>,
}

impl FakeCredentialResolver {
    pub fn new(values: BTreeMap<CredentialReference, Vec<u8>>) -> Self {
        Self { values }
    }
}

impl CredentialResolver for FakeCredentialResolver {
    fn resolve(
        &mut self,
        reference: &CredentialReference,
    ) -> Result<SecretValue, CredentialResolveError> {
        let value = self
            .values
            .get(reference)
            .ok_or(CredentialResolveError::Unavailable)?;
        SecretValue::new(value.clone())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum CredentialResolveError {
    #[error("credential is unavailable")]
    Unavailable,
    #[error("credential provider returned an empty value")]
    EmptyValue,
    #[error("credential provider failed")]
    ProviderFailed,
    #[error("credential provider returned an invalid response")]
    InvalidProviderResponse,
    #[error("credential reference is not supported by this resolver")]
    UnsupportedReference,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
#[error("credential provider command failed")]
pub struct BwsCommandError;

/// Boundary around the BWS process. Arguments are individual argv entries, never shell text.
pub trait BwsCommandRunner {
    fn run(&mut self, args: &[String]) -> Result<Vec<u8>, BwsCommandError>;
}

pub struct ProcessBwsRunner {
    executable: PathBuf,
}

impl ProcessBwsRunner {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
        }
    }
}

impl BwsCommandRunner for ProcessBwsRunner {
    fn run(&mut self, args: &[String]) -> Result<Vec<u8>, BwsCommandError> {
        let output = Command::new(&self.executable)
            .args(args)
            .output()
            .map_err(|_| BwsCommandError)?;
        if !output.status.success() {
            return Err(BwsCommandError);
        }
        Ok(output.stdout)
    }
}

pub struct BwsCredentialResolver<R> {
    runner: R,
}

impl<R> BwsCredentialResolver<R> {
    pub fn new(runner: R) -> Self {
        Self { runner }
    }

    pub fn runner(&self) -> &R {
        &self.runner
    }
}

impl<R: BwsCommandRunner> CredentialResolver for BwsCredentialResolver<R> {
    fn resolve(
        &mut self,
        reference: &CredentialReference,
    ) -> Result<SecretValue, CredentialResolveError> {
        if reference.scheme() != "bws" {
            return Err(CredentialResolveError::UnsupportedReference);
        }
        let args = vec![
            "secret".to_string(),
            "get".to_string(),
            reference.opaque().to_string(),
        ];
        let bytes = self
            .runner
            .run(&args)
            .map_err(|_| CredentialResolveError::ProviderFailed)?;
        let response: BwsResponse = serde_json::from_slice(&bytes)
            .map_err(|_| CredentialResolveError::InvalidProviderResponse)?;
        SecretValue::new(response.value.into_bytes())
    }
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct BwsResponse {
    value: String,
}

/// Capability-rooted storage for credential material that must exist on one target.
pub struct LocalSensitiveFileStore {
    root: Dir,
    root_path: PathBuf,
    directory_sync: Arc<SensitiveDirectorySync>,
}

type SensitiveDirectorySync = dyn Fn(&Path) -> Result<(), std::io::Error> + Send + Sync;

impl LocalSensitiveFileStore {
    pub fn open(root: &std::path::Path) -> Result<Self, SensitiveFileError> {
        Self::open_with_directory_sync(root, Arc::new(sync_sensitive_directory))
    }

    /// Test seam for proving that directory-metadata durability failures are surfaced.
    #[doc(hidden)]
    pub fn open_with_directory_sync(
        root: &std::path::Path,
        directory_sync: Arc<SensitiveDirectorySync>,
    ) -> Result<Self, SensitiveFileError> {
        if !root.is_absolute() || root.parent().is_none() {
            return Err(SensitiveFileError::UnsafePath);
        }
        let root_path = root.to_path_buf();
        let metadata = std::fs::symlink_metadata(root).map_err(|_| SensitiveFileError::Io)?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(SensitiveFileError::UnsafePath);
        }
        #[cfg(unix)]
        std::fs::set_permissions(root, std::os::unix::fs::PermissionsExt::from_mode(0o700))
            .map_err(|_| SensitiveFileError::Io)?;
        let root =
            Dir::open_ambient_dir(root, ambient_authority()).map_err(|_| SensitiveFileError::Io)?;
        Ok(Self {
            root,
            root_path,
            directory_sync,
        })
    }

    pub fn write(
        &self,
        path: &crate::NormalizedManagedPath,
        value: &SecretValue,
    ) -> Result<(), SensitiveFileError> {
        self.write_bytes(path, value.expose_for_apply())
    }

    /// Restores integrity-checked recovery bytes. Unlike a provisioned credential, a valid
    /// pre-transaction file may be empty.
    pub fn restore_backup_bytes(
        &self,
        path: &crate::NormalizedManagedPath,
        bytes: &[u8],
    ) -> Result<(), SensitiveFileError> {
        self.write_bytes(path, bytes)
    }

    fn write_bytes(
        &self,
        path: &crate::NormalizedManagedPath,
        bytes: &[u8],
    ) -> Result<(), SensitiveFileError> {
        let components = path.as_str().split('/').collect::<Vec<_>>();
        let mut parent = PathBuf::new();
        for component in components.iter().take(components.len().saturating_sub(1)) {
            let containing_parent = parent.clone();
            parent.push(component);
            match self.root.symlink_metadata(&parent) {
                Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_dir() => {
                    return Err(SensitiveFileError::UnsafePath);
                }
                Ok(_) => {}
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                    self.root
                        .create_dir(&parent)
                        .map_err(|_| SensitiveFileError::Io)?;
                    #[cfg(unix)]
                    self.root
                        .set_permissions(&parent, Permissions::from_mode(0o700))
                        .map_err(|_| SensitiveFileError::Io)?;
                    (self.directory_sync)(&self.root_path.join(&containing_parent))
                        .map_err(|_| SensitiveFileError::Io)?;
                }
                Err(_) => return Err(SensitiveFileError::Io),
            }
            #[cfg(unix)]
            self.root
                .set_permissions(&parent, Permissions::from_mode(0o700))
                .map_err(|_| SensitiveFileError::Io)?;
        }
        match self.root.symlink_metadata(path.as_str()) {
            Ok(metadata) if metadata.file_type().is_symlink() || !metadata.is_file() => {
                return Err(SensitiveFileError::UnsafePath);
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(_) => return Err(SensitiveFileError::Io),
        }
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = self
            .root
            .open_with(path.as_str(), &options)
            .map_err(|_| SensitiveFileError::Io)?;
        #[cfg(unix)]
        file.set_permissions(Permissions::from_mode(0o600))
            .map_err(|_| SensitiveFileError::Io)?;
        file.write_all(bytes).map_err(|_| SensitiveFileError::Io)?;
        file.sync_all().map_err(|_| SensitiveFileError::Io)?;
        (self.directory_sync)(&self.root_path.join(parent)).map_err(|_| SensitiveFileError::Io)
    }
}

#[cfg(unix)]
fn sync_sensitive_directory(path: &Path) -> Result<(), std::io::Error> {
    std::fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_sensitive_directory(path: &Path) -> Result<(), std::io::Error> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?
        .sync_all()
}

#[cfg(not(any(unix, windows)))]
fn sync_sensitive_directory(path: &Path) -> Result<(), std::io::Error> {
    std::fs::File::open(path)?.sync_all()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum SensitiveFileError {
    #[error("unsafe sensitive-file path")]
    UnsafePath,
    #[error("sensitive-file operation failed")]
    Io,
}
