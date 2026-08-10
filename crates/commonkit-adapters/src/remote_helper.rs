use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use commonkit_contracts::StableId;
use commonkit_core::{RootAccess, TargetRoot};

use crate::{
    ArtifactStore, ContentSensitivity, LocalTargetFilesystem, SshFilesystemRequest,
    SshFilesystemResponse, TargetFilesystem, TargetFilesystemError,
};

pub struct TargetHelper {
    roots: BTreeMap<StableId, (PathBuf, RootAccess)>,
    artifacts: ArtifactStore,
    receipts: PathBuf,
}

impl TargetHelper {
    pub fn open(roots: Vec<TargetRoot>, state_root: &Path) -> Result<Self, TargetFilesystemError> {
        let mut mapped = BTreeMap::new();
        for root in roots {
            let path = PathBuf::from(&root.path);
            if mapped.insert(root.id, (path, root.access)).is_some() {
                return Err(TargetFilesystemError::InvalidSshConfig("duplicate root id"));
            }
        }
        let artifacts = ArtifactStore::open(state_root.join("artifacts"))
            .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
        let receipts = state_root.join("receipts");
        std::fs::create_dir_all(&receipts)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&receipts, std::fs::Permissions::from_mode(0o700))?;
        }
        Ok(Self {
            roots: mapped,
            artifacts,
            receipts,
        })
    }

    fn root(&self, id: &StableId) -> Result<LocalTargetFilesystem, TargetFilesystemError> {
        let (path, access) = self
            .roots
            .get(id)
            .ok_or_else(|| TargetFilesystemError::UnknownRoot(id.clone()))?;
        LocalTargetFilesystem::open(path, *access)
    }

    pub fn dispatch(
        &self,
        request: SshFilesystemRequest,
    ) -> Result<SshFilesystemResponse, TargetFilesystemError> {
        match request {
            SshFilesystemRequest::ReadFile { root_id, path } => {
                Ok(match self.root(&root_id)?.read_file(&path)? {
                    Some(content) => SshFilesystemResponse::File { content },
                    None => SshFilesystemResponse::Absent,
                })
            }
            SshFilesystemRequest::InspectResource { root_id, path } => {
                Ok(SshFilesystemResponse::Resource {
                    resource: self.root(&root_id)?.inspect_resource(&path)?,
                })
            }
            SshFilesystemRequest::WriteFile {
                root_id,
                path,
                content,
            } => {
                self.root(&root_id)?.write_file(&path, &content)?;
                Ok(SshFilesystemResponse::Applied)
            }
            SshFilesystemRequest::WriteDirectory {
                root_id,
                path,
                mode,
            } => {
                self.root(&root_id)?.write_directory(&path, mode.as_ref())?;
                Ok(SshFilesystemResponse::Applied)
            }
            SshFilesystemRequest::WriteSymlink {
                root_id,
                path,
                target,
                target_kind,
            } => {
                self.root(&root_id)?
                    .write_symlink(&path, &target, target_kind)?;
                Ok(SshFilesystemResponse::Applied)
            }
            SshFilesystemRequest::Remove { root_id, path } => {
                self.root(&root_id)?.remove_resource(&path)?;
                Ok(SshFilesystemResponse::Applied)
            }
            SshFilesystemRequest::StageArtifact {
                digest, content, ..
            } => {
                let reference = self
                    .artifacts
                    .put(&content, ContentSensitivity::Portable)
                    .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
                if reference.digest != digest {
                    return Err(TargetFilesystemError::RemoteArtifact);
                }
                Ok(SshFilesystemResponse::ArtifactStaged { digest })
            }
            SshFilesystemRequest::VerifyArtifact { digest, .. } => {
                self.artifacts
                    .verify_digest(&digest)
                    .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
                Ok(SshFilesystemResponse::ArtifactVerified { digest })
            }
            SshFilesystemRequest::BindRecoveryReceipt {
                run_id,
                receipt_digest,
            } => {
                self.artifacts
                    .verify_digest(&receipt_digest)
                    .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
                let path = self.receipts.join(format!("{}.digest", run_id.as_str()));
                let bytes = receipt_digest.as_str().as_bytes();
                let mut options = std::fs::OpenOptions::new();
                options.write(true).create_new(true);
                #[cfg(unix)]
                {
                    use std::os::unix::fs::OpenOptionsExt;
                    options.mode(0o600);
                }
                match options.open(&path) {
                    Ok(mut file) => {
                        use std::io::Write;
                        file.write_all(bytes)?;
                        file.sync_all()?;
                    }
                    Err(error)
                        if error.kind() == std::io::ErrorKind::AlreadyExists
                            && std::fs::read(&path)? == bytes => {}
                    Err(error) => return Err(error.into()),
                }
                Ok(SshFilesystemResponse::RecoveryReceiptBound { receipt_digest })
            }
            SshFilesystemRequest::RecoverRun {
                run_id,
                receipt_digest,
            } => {
                self.artifacts
                    .verify_digest(&receipt_digest)
                    .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
                let bound =
                    std::fs::read(self.receipts.join(format!("{}.digest", run_id.as_str())))?;
                if bound != receipt_digest.as_str().as_bytes() {
                    return Err(TargetFilesystemError::RemoteArtifact);
                }
                Ok(SshFilesystemResponse::RecoveryReady { receipt_digest })
            }
        }
    }
}
