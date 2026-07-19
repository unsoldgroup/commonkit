use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

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

#[derive(Debug, Clone)]
pub struct ArtifactStore {
    root: PathBuf,
}

impl ArtifactStore {
    pub fn open(root: impl AsRef<Path>) -> Result<Self, ArtifactError> {
        fs::create_dir_all(root.as_ref())?;
        set_private_directory(root.as_ref())?;
        let root = root.as_ref().canonicalize()?;
        if !root.is_dir() {
            return Err(ArtifactError::InvalidRoot);
        }
        Ok(Self { root })
    }

    /// Opens a store for integrity validation without changing the filesystem.
    ///
    /// Unlike [`Self::open`], this never creates the root or repairs its
    /// permissions. Execution-time authority checks use this path so rejecting
    /// a stale or malformed plan is a strictly read-only operation.
    pub fn open_existing(root: impl AsRef<Path>) -> Result<Self, ArtifactError> {
        let root = root.as_ref();
        let metadata = fs::symlink_metadata(root).map_err(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                ArtifactError::InvalidRoot
            } else {
                error.into()
            }
        })?;
        if metadata.file_type().is_symlink() || !metadata.is_dir() {
            return Err(ArtifactError::InvalidRoot);
        }
        if !has_private_directory_permissions(&metadata) {
            return Err(ArtifactError::InvalidRoot);
        }
        let root = root.canonicalize()?;
        Ok(Self { root })
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
        let path = self.path_for(&reference.digest);
        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&path) {
            Ok(mut file) => {
                file.write_all(bytes)?;
                file.sync_all()?;
                sync_directory(&self.root)?;
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                self.load(&reference)?;
            }
            Err(error) => return Err(error.into()),
        }
        Ok(reference)
    }

    pub fn load(&self, reference: &ContentReference) -> Result<Vec<u8>, ArtifactError> {
        let path = self.path_for(&reference.digest);
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(ArtifactError::UnsafeArtifactType);
        }
        let mut file = OpenOptions::new().read(true).open(path)?;
        let mut bytes = Vec::new();
        file.read_to_end(&mut bytes)?;
        if u64::try_from(bytes.len()).ok() != Some(reference.bytes)
            || digest_bytes(&bytes)? != reference.digest
        {
            return Err(ArtifactError::DigestMismatch);
        }
        Ok(bytes)
    }

    pub fn verify_digest(&self, digest: &Sha256Digest) -> Result<(), ArtifactError> {
        let path = self.path_for(digest);
        let metadata = fs::symlink_metadata(&path)?;
        if metadata.file_type().is_symlink() || !metadata.is_file() {
            return Err(ArtifactError::UnsafeArtifactType);
        }
        let reference = ContentReference {
            digest: digest.clone(),
            bytes: metadata.len(),
            sensitivity: ContentSensitivity::Portable,
        };
        self.load(&reference).map(|_| ())
    }

    fn path_for(&self, digest: &Sha256Digest) -> PathBuf {
        self.root.join(format!(
            "{}.blob",
            digest.as_str().trim_start_matches("sha256:")
        ))
    }
}

#[cfg(unix)]
fn has_private_directory_permissions(metadata: &fs::Metadata) -> bool {
    use std::os::unix::fs::PermissionsExt;

    metadata.permissions().mode() & 0o777 == 0o700
}

#[cfg(not(unix))]
fn has_private_directory_permissions(_metadata: &fs::Metadata) -> bool {
    true
}

fn digest_bytes(bytes: &[u8]) -> Result<Sha256Digest, ArtifactError> {
    let digest = Sha256::digest(bytes);
    Sha256Digest::parse(format!("sha256:{digest:x}")).map_err(ArtifactError::Contract)
}

#[cfg(unix)]
fn set_private_directory(path: &Path) -> Result<(), std::io::Error> {
    use std::os::unix::fs::PermissionsExt;

    fs::set_permissions(path, fs::Permissions::from_mode(0o700))
}

#[cfg(not(unix))]
fn set_private_directory(_path: &Path) -> Result<(), std::io::Error> {
    Ok(())
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), std::io::Error> {
    fs::File::open(path)?.sync_all()
}

#[cfg(not(unix))]
fn sync_directory(_path: &Path) -> Result<(), std::io::Error> {
    Ok(())
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
