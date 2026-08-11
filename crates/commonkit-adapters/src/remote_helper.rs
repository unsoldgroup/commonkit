use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use commonkit_contracts::StableId;
use commonkit_core::{RootAccess, TargetRoot};

use crate::{
    ArtifactStore, ContentSensitivity, EngramTargetSyncMode, LocalTargetFilesystem,
    PackageMutationPhase, ProcessOfflinePackageBackend, SshFilesystemRequest,
    SshFilesystemResponse, TargetFilesystem, TargetFilesystemError,
};

pub struct TargetHelper {
    roots: BTreeMap<StableId, (PathBuf, RootAccess)>,
    artifacts: ArtifactStore,
    receipts: PathBuf,
    engram_executable: Option<PathBuf>,
}

impl TargetHelper {
    pub fn open(
        roots: Vec<TargetRoot>,
        state_root: &Path,
        engram_executable: Option<PathBuf>,
    ) -> Result<Self, TargetFilesystemError> {
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
        if let Some(executable) = &engram_executable {
            let metadata = std::fs::symlink_metadata(executable)?;
            if !executable.is_absolute() || metadata.file_type().is_symlink() || !metadata.is_file()
            {
                return Err(TargetFilesystemError::InvalidEngramExecutable);
            }
        }
        Ok(Self {
            roots: mapped,
            artifacts,
            receipts,
            engram_executable,
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
            SshFilesystemRequest::EngramSync {
                root_id,
                project_id,
                project_path,
                mode,
            } => {
                let (root_path, _) = self
                    .roots
                    .get(&root_id)
                    .ok_or_else(|| TargetFilesystemError::UnknownRoot(root_id.clone()))?;
                self.root(&root_id)?;
                let mut project = root_path.clone();
                let mut relative = PathBuf::new();
                for component in project_path.as_str().split('/') {
                    relative.push(component);
                    project.push(component);
                    let metadata = std::fs::symlink_metadata(&project)?;
                    if metadata.file_type().is_symlink() {
                        return Err(TargetFilesystemError::SymlinkEncountered(
                            relative.to_string_lossy().into_owned(),
                        ));
                    }
                    if !metadata.is_dir() {
                        return Err(TargetFilesystemError::NotDirectory(
                            relative.to_string_lossy().into_owned(),
                        ));
                    }
                }
                let executable = self
                    .engram_executable
                    .as_ref()
                    .ok_or(TargetFilesystemError::EngramExecutableUnavailable)?;
                let mut command = std::process::Command::new(executable);
                command
                    .arg("sync")
                    .arg("--project")
                    .arg(project_id.as_str())
                    .current_dir(project)
                    .stdout(std::process::Stdio::null())
                    .stderr(std::process::Stdio::null());
                if mode == EngramTargetSyncMode::Import {
                    command.arg("--import");
                }
                let status = command.status()?;
                if !status.success() {
                    return Err(TargetFilesystemError::EngramCommandFailed);
                }
                Ok(SshFilesystemResponse::EngramSynced { mode })
            }
            SshFilesystemRequest::ListDirectory { root_id, path } => {
                Ok(SshFilesystemResponse::Directory {
                    entries: self.root(&root_id)?.list_directory(&path)?,
                })
            }
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
            SshFilesystemRequest::PackageMutation {
                root_id,
                phase,
                resolution,
                artifacts,
            } => {
                let (root_path, access) = self
                    .roots
                    .get(&root_id)
                    .ok_or_else(|| TargetFilesystemError::UnknownRoot(root_id.clone()))?;
                if phase != PackageMutationPhase::Observe && access != &RootAccess::ReadWrite {
                    return Err(TargetFilesystemError::ReadOnly);
                }
                for artifact in artifacts {
                    let stored = self
                        .artifacts
                        .put(&artifact.bytes, artifact.reference.sensitivity)
                        .map_err(|_| TargetFilesystemError::RemoteArtifact)?;
                    if stored.digest != artifact.reference.digest
                        || stored.bytes != artifact.reference.bytes
                    {
                        return Err(TargetFilesystemError::RemoteArtifact);
                    }
                }
                let mut backend = ProcessOfflinePackageBackend::new(root_path);
                match phase {
                    PackageMutationPhase::Observe => {
                        let observed =
                            crate::PackageMutationBackend::observe(&mut backend, &resolution)
                                .map_err(|_| TargetFilesystemError::PackageCommandFailed)?;
                        Ok(SshFilesystemResponse::PackageObserved {
                            installed_versions: observed.installed_versions,
                        })
                    }
                    PackageMutationPhase::Prepare => {
                        crate::PackageMutationBackend::prepare_offline(
                            &mut backend,
                            &resolution,
                            &self.artifacts,
                        )
                        .map_err(|_| TargetFilesystemError::PackageCommandFailed)?;
                        Ok(SshFilesystemResponse::Applied)
                    }
                    PackageMutationPhase::Apply => {
                        crate::PackageMutationBackend::apply_offline(
                            &mut backend,
                            &resolution,
                            &self.artifacts,
                        )
                        .map_err(|_| TargetFilesystemError::PackageCommandFailed)?;
                        Ok(SshFilesystemResponse::Applied)
                    }
                    PackageMutationPhase::Verify => {
                        crate::PackageMutationBackend::verify_offline(
                            &mut backend,
                            &resolution,
                            &self.artifacts,
                        )
                        .map_err(|_| TargetFilesystemError::PackageCommandFailed)?;
                        Ok(SshFilesystemResponse::Applied)
                    }
                }
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

#[cfg(all(test, unix))]
mod tests {
    use std::fs;
    use std::os::unix::fs::symlink;
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::*;
    use crate::NormalizedManagedPath;

    fn temp(name: &str) -> PathBuf {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!(
            "commonkit-helper-{name}-{}-{nonce}",
            std::process::id()
        ))
    }

    #[test]
    fn engram_sync_rejects_a_symlinked_project_directory() {
        let root = temp("root");
        let state = temp("state");
        let project = root.join("real-project");
        fs::create_dir_all(&project).unwrap();
        symlink(&project, root.join("linked-project")).unwrap();
        let root_id = StableId::parse("home").unwrap();
        let helper = TargetHelper::open(
            vec![TargetRoot {
                id: root_id.clone(),
                path: root.to_string_lossy().into_owned(),
                access: RootAccess::ReadWrite,
            }],
            &state,
            None,
        )
        .unwrap();

        let result = helper.dispatch(SshFilesystemRequest::EngramSync {
            root_id,
            project_id: "github.com/unsoldgroup/commonkit".try_into().unwrap(),
            project_path: NormalizedManagedPath::parse("linked-project").unwrap(),
            mode: EngramTargetSyncMode::Export,
        });

        assert!(matches!(
            result,
            Err(TargetFilesystemError::SymlinkEncountered(path)) if path == "linked-project"
        ));
        fs::remove_dir_all(root).unwrap();
        fs::remove_dir_all(state).unwrap();
    }
}
