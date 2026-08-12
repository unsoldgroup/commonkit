use std::collections::{BTreeMap, BTreeSet};
use std::fs::OpenOptions;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;

use commonkit_contracts::{Sha256Digest, StableId};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::{
    EngramTargetRuntime, EngramTargetSyncMode, NormalizedManagedPath, TargetFilesystem,
    TargetFilesystemError,
};

pub const MAX_ENGRAM_CHUNK_BYTES: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EngramProjectId(String);

impl TryFrom<String> for EngramProjectId {
    type Error = EngramError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty()
            || value.starts_with('/')
            || value.ends_with('/')
            || value.contains(char::is_whitespace)
            || value
                .split('/')
                .any(|part| part.is_empty() || part == "." || part == "..")
        {
            return Err(EngramError::InvalidProjectId(value));
        }
        Ok(Self(value))
    }
}

impl TryFrom<&str> for EngramProjectId {
    type Error = EngramError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.to_owned().try_into()
    }
}

impl From<EngramProjectId> for String {
    fn from(value: EngramProjectId) -> Self {
        value.0
    }
}

impl EngramProjectId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EngramOwnerId(String);

impl TryFrom<String> for EngramOwnerId {
    type Error = EngramError;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        if value.is_empty()
            || value.contains(char::is_whitespace)
            || value
                .strip_prefix("github:")
                .is_none_or(|principal| principal.is_empty())
        {
            Err(EngramError::InvalidOwnerId(value))
        } else {
            Ok(Self(value))
        }
    }
}

impl TryFrom<&str> for EngramOwnerId {
    type Error = EngramError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        value.to_owned().try_into()
    }
}

impl From<EngramOwnerId> for String {
    fn from(value: EngramOwnerId) -> Self {
        value.0
    }
}

impl EngramOwnerId {
    pub fn from_principal(principal: &commonkit_contracts::Principal) -> Self {
        Self(format!("github:{}", principal.as_str()))
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngramScope {
    Project,
    Personal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngramGrantState {
    Active,
    Withdrawn,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EngramGrant {
    pub id: StableId,
    pub project_id: EngramProjectId,
    pub grantor: EngramOwnerId,
    pub grantee: EngramOwnerId,
    pub state: EngramGrantState,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EngramScopeAttestation {
    schema: String,
    exporter_version: String,
    project_id: EngramProjectId,
    scope: EngramScope,
    manifest_digest: Sha256Digest,
    chunks: Vec<EngramChunkDigest>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EngramChunkSetDeclaration {
    pub project_id: EngramProjectId,
    pub owner_id: EngramOwnerId,
    pub root: PathBuf,
    pub scope: EngramScope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EngramTargetChunkSetDeclaration {
    pub target_id: StableId,
    pub project_id: EngramProjectId,
    pub owner_id: EngramOwnerId,
    pub project_root: NormalizedManagedPath,
    pub root: NormalizedManagedPath,
    pub scope: EngramScope,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EngramChunkSetState {
    InSync,
    Drifted,
    Unreachable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EngramChunkDigest {
    pub id: String,
    pub digest: Sha256Digest,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EngramChunkSetStatus {
    pub project_id: EngramProjectId,
    pub owner_id: EngramOwnerId,
    pub state: EngramChunkSetState,
    pub present: Vec<EngramChunkDigest>,
    pub missing: BTreeSet<String>,
    pub extra: BTreeSet<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EngramChunkMovement {
    pub chunk_id: String,
    pub digest: Sha256Digest,
    pub bytes: u64,
    pub from: PathBuf,
    pub to: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EngramReconciliationReceipt {
    pub project_id: EngramProjectId,
    pub moved: Vec<EngramChunkMovement>,
    pub unresolved: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EngramTargetChunkMovement {
    pub chunk_id: String,
    pub digest: Sha256Digest,
    pub bytes: u64,
    pub from_target: StableId,
    pub to_target: StableId,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EngramTargetReconciliationReceipt {
    pub project_id: EngramProjectId,
    pub moved: Vec<EngramTargetChunkMovement>,
    pub unresolved: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EngramCommandOutput {
    pub success: bool,
}

pub trait EngramCommandRunner: Send + Sync {
    fn run(
        &self,
        working_directory: &Path,
        arguments: &[&str],
    ) -> Result<EngramCommandOutput, EngramError>;
}

#[derive(Debug, Clone)]
pub struct ProcessEngramCommandRunner {
    executable: PathBuf,
    stable_directory: Option<Arc<tempfile::TempDir>>,
}

impl ProcessEngramCommandRunner {
    pub fn from_path(executable: impl Into<PathBuf>) -> Self {
        let executable = executable.into();
        let resolved = resolve_executable_path(&executable).unwrap_or_else(|| executable.clone());
        match stable_executable(&resolved) {
            Ok((executable, stable_directory)) => Self {
                executable,
                stable_directory: Some(Arc::new(stable_directory)),
            },
            Err(_) => Self {
                executable: resolved,
                stable_directory: None,
            },
        }
    }

    pub fn try_from_path(executable: impl Into<PathBuf>) -> Result<Self, std::io::Error> {
        let executable = executable.into();
        let resolved = resolve_executable_path(&executable).unwrap_or(executable);
        let (executable, stable_directory) = stable_executable(&resolved)?;
        Ok(Self {
            executable,
            stable_directory: Some(Arc::new(stable_directory)),
        })
    }

    #[cfg(unix)]
    pub fn run_in_directory_handle(
        &self,
        directory: &std::fs::File,
        arguments: &[&str],
    ) -> Result<EngramCommandOutput, EngramError> {
        use std::os::fd::AsRawFd;
        use std::os::unix::process::CommandExt;

        if self.stable_directory.is_none() {
            return Err(
                std::io::Error::new(std::io::ErrorKind::NotFound, "Engram executable").into(),
            );
        }
        let directory = directory.try_clone()?;
        let directory_fd = directory.as_raw_fd();
        clear_close_on_exec(directory_fd)?;
        let mut command = Command::new(&self.executable);
        command
            .args(arguments)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        unsafe {
            command.pre_exec(move || {
                if libc::fchdir(directory_fd) == -1 {
                    return Err(std::io::Error::last_os_error());
                }
                Ok(())
            });
        }
        let status = command.status()?;
        Ok(EngramCommandOutput {
            success: status.success(),
        })
    }
}

impl Default for ProcessEngramCommandRunner {
    fn default() -> Self {
        Self::from_path("engram")
    }
}

impl EngramCommandRunner for ProcessEngramCommandRunner {
    fn run(
        &self,
        working_directory: &Path,
        arguments: &[&str],
    ) -> Result<EngramCommandOutput, EngramError> {
        #[cfg(unix)]
        {
            let directory = open_directory_handle(working_directory)?;
            self.run_in_directory_handle(&directory, arguments)
        }
        #[cfg(not(unix))]
        {
            if self.stable_directory.is_none() {
                return Err(
                    std::io::Error::new(std::io::ErrorKind::NotFound, "Engram executable").into(),
                );
            }
            let status = Command::new(&self.executable)
                .args(arguments)
                .current_dir(working_directory)
                .status()?;
            Ok(EngramCommandOutput {
                success: status.success(),
            })
        }
    }
}

fn resolve_executable_path(path: &Path) -> Option<PathBuf> {
    if path.is_absolute() {
        return Some(path.to_path_buf());
    }
    let paths = std::env::var_os("PATH")?;
    std::env::split_paths(&paths)
        .map(|directory| directory.join(path))
        .find(|candidate| candidate.is_file())
}

#[cfg(not(unix))]
fn stable_executable(path: &Path) -> Result<(PathBuf, tempfile::TempDir), std::io::Error> {
    let metadata = std::fs::symlink_metadata(path)?;
    if !metadata.is_file() {
        return Err(std::io::Error::other(
            "Engram executable is not a regular file",
        ));
    }
    let directory = tempfile::Builder::new()
        .prefix("commonkit-engram-exec-")
        .tempdir()?;
    let executable = directory.path().join("engram");
    std::fs::copy(path, &executable)?;
    Ok((executable, directory))
}

#[cfg(unix)]
fn stable_executable(path: &Path) -> Result<(PathBuf, tempfile::TempDir), std::io::Error> {
    use std::os::unix::fs::OpenOptionsExt;
    let mut options = OpenOptions::new();
    options.read(true).custom_flags(libc::O_NOFOLLOW);
    let file = options.open(path)?;
    let metadata = file.metadata()?;
    if !metadata.is_file() {
        return Err(std::io::Error::other(
            "Engram executable is not a regular file",
        ));
    }
    let directory = tempfile::Builder::new()
        .prefix("commonkit-engram-exec-")
        .tempdir()?;
    let executable = directory.path().join("engram");
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&executable)?;
    let mut source = file;
    std::io::copy(&mut source, &mut output)?;
    output.sync_all()?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(
            &executable,
            std::fs::Permissions::from_mode(metadata.permissions().mode()),
        )?;
    }
    Ok((executable, directory))
}

#[cfg(unix)]
fn clear_close_on_exec(fd: std::os::fd::RawFd) -> Result<(), std::io::Error> {
    let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
    if flags == -1 {
        return Err(std::io::Error::last_os_error());
    }
    if unsafe { libc::fcntl(fd, libc::F_SETFD, flags & !libc::FD_CLOEXEC) } == -1 {
        return Err(std::io::Error::last_os_error());
    }
    Ok(())
}

#[cfg(unix)]
fn open_directory_handle(path: &Path) -> Result<std::fs::File, std::io::Error> {
    let current = open_engram_root(path)?;
    Ok(current.into_std_file())
}

fn open_engram_root(path: &Path) -> Result<cap_std::fs::Dir, std::io::Error> {
    match crate::target::open_absolute_directory_nofollow(path) {
        Ok(directory) => Ok(directory),
        Err(error) if cfg!(target_os = "macos") => {
            let translated = ["/var", "/tmp"].iter().find_map(|prefix| {
                path.strip_prefix(prefix).ok().map(|suffix| {
                    Path::new("/private")
                        .join(prefix.strip_prefix('/').unwrap())
                        .join(suffix)
                })
            });
            match translated {
                Some(path) => crate::target::open_absolute_directory_nofollow(&path),
                None => Err(error),
            }
        }
        Err(error) => Err(error),
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EngramManifest {
    version: u32,
    chunks: Vec<EngramManifestChunk>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct EngramManifestChunk {
    id: String,
    created_by: String,
    created_at: String,
    sessions: u64,
    memories: u64,
    prompts: u64,
}

pub struct EngramChunkAdapter;

impl EngramChunkAdapter {
    pub fn status(
        declaration: &EngramChunkSetDeclaration,
    ) -> Result<EngramChunkSetStatus, EngramError> {
        recover_local_stage_files(&declaration.root)?;
        let manifest = read_manifest(&declaration.root)?;
        let declared = manifest
            .chunks
            .iter()
            .map(|chunk| chunk.id.clone())
            .collect::<BTreeSet<_>>();
        let observed = observed_chunk_ids(&declaration.root)?;
        let missing: BTreeSet<String> = declared.difference(&observed).cloned().collect();
        let extra: BTreeSet<String> = observed.difference(&declared).cloned().collect();
        let mut present = Vec::new();
        for id in declared.intersection(&observed) {
            present.push(chunk_digest(&declaration.root, id)?);
        }
        let state = if missing.is_empty() && extra.is_empty() {
            EngramChunkSetState::InSync
        } else {
            EngramChunkSetState::Drifted
        };
        Ok(EngramChunkSetStatus {
            project_id: declaration.project_id.clone(),
            owner_id: declaration.owner_id.clone(),
            state,
            present,
            missing,
            extra,
        })
    }

    pub fn reconcile(
        left: &EngramChunkSetDeclaration,
        right: &EngramChunkSetDeclaration,
    ) -> Result<EngramReconciliationReceipt, EngramError> {
        if left.scope == EngramScope::Personal || right.scope == EngramScope::Personal {
            return Err(EngramError::PersonalTransport);
        }
        if left.project_id != right.project_id {
            return Err(EngramError::ProjectIdentityMismatch);
        }
        if left.owner_id != right.owner_id {
            return Err(EngramError::MissingScopeAttestation);
        }
        if !left.root.is_dir() || !right.root.is_dir() {
            return Ok(EngramReconciliationReceipt {
                project_id: left.project_id.clone(),
                moved: Vec::new(),
                unresolved: Some("peer chunk set is unreachable".to_owned()),
            });
        }
        recover_local_stage_files(&left.root)?;
        recover_local_stage_files(&right.root)?;
        let left_manifest = read_manifest(&left.root)?;
        let right_manifest = read_manifest(&right.root)?;
        let left_chunks = manifest_map(&left_manifest)?;
        let right_chunks = manifest_map(&right_manifest)?;
        let mut moved = Vec::new();
        for id in left_chunks
            .keys()
            .filter(|id| !right_chunks.contains_key(*id))
        {
            moved.push(copy_chunk(&left.root, &right.root, id)?);
        }
        for id in right_chunks
            .keys()
            .filter(|id| !left_chunks.contains_key(*id))
        {
            moved.push(copy_chunk(&right.root, &left.root, id)?);
        }
        for id in left_chunks
            .keys()
            .filter(|id| right_chunks.contains_key(*id))
        {
            let left_digest = chunk_digest(&left.root, id)?;
            let right_digest = chunk_digest(&right.root, id)?;
            if left_digest.digest != right_digest.digest {
                return Err(EngramError::ChunkCollision(id.clone()));
            }
        }
        let merged = merge_manifests(left_manifest, right_manifest)?;
        write_manifest(&left.root, &merged)?;
        write_manifest(&right.root, &merged)?;
        Ok(EngramReconciliationReceipt {
            project_id: left.project_id.clone(),
            moved,
            unresolved: None,
        })
    }

    pub fn reconcile_installed(
        runner: &dyn EngramCommandRunner,
        left: &EngramChunkSetDeclaration,
        right: &EngramChunkSetDeclaration,
    ) -> Result<EngramReconciliationReceipt, EngramError> {
        let left_project = left.root.parent().ok_or(EngramError::MissingProjectRoot)?;
        let right_project = right.root.parent().ok_or(EngramError::MissingProjectRoot)?;
        run_engram(
            runner,
            left_project,
            &["sync", "--project", left.project_id.as_str()],
        )?;
        run_engram(
            runner,
            right_project,
            &["sync", "--project", right.project_id.as_str()],
        )?;
        let receipt = Self::reconcile(left, right)?;
        if receipt.unresolved.is_none() {
            run_engram(
                runner,
                left_project,
                &["sync", "--import", "--project", left.project_id.as_str()],
            )?;
            run_engram(
                runner,
                right_project,
                &["sync", "--import", "--project", right.project_id.as_str()],
            )?;
        }
        Ok(receipt)
    }

    pub fn status_target(
        filesystem: &dyn TargetFilesystem,
        declaration: &EngramTargetChunkSetDeclaration,
    ) -> Result<EngramChunkSetStatus, EngramError> {
        recover_target_stage_files(filesystem, &declaration.root)?;
        let manifest = match read_target_manifest(filesystem, &declaration.root) {
            Ok(manifest) => manifest,
            Err(EngramError::TargetUnreachable) => {
                return Ok(EngramChunkSetStatus {
                    project_id: declaration.project_id.clone(),
                    owner_id: declaration.owner_id.clone(),
                    state: EngramChunkSetState::Unreachable,
                    present: Vec::new(),
                    missing: BTreeSet::new(),
                    extra: BTreeSet::new(),
                });
            }
            Err(error) => return Err(error),
        };
        let declared = manifest
            .chunks
            .iter()
            .map(|chunk| chunk.id.clone())
            .collect::<BTreeSet<_>>();
        let observed = observed_target_chunk_ids(filesystem, &declaration.root)?;
        let missing: BTreeSet<String> = declared.difference(&observed).cloned().collect();
        let extra: BTreeSet<String> = observed.difference(&declared).cloned().collect();
        let mut present = Vec::new();
        for id in declared.intersection(&observed) {
            let bytes = read_target_chunk(filesystem, &declaration.root, id)?;
            present.push(digest_chunk_bytes(id, &bytes)?);
        }
        Ok(EngramChunkSetStatus {
            project_id: declaration.project_id.clone(),
            owner_id: declaration.owner_id.clone(),
            state: if missing.is_empty() && extra.is_empty() {
                EngramChunkSetState::InSync
            } else {
                EngramChunkSetState::Drifted
            },
            present,
            missing,
            extra,
        })
    }

    pub fn reconcile_targets(
        left_filesystem: &dyn TargetFilesystem,
        left: &EngramTargetChunkSetDeclaration,
        right_filesystem: &dyn TargetFilesystem,
        right: &EngramTargetChunkSetDeclaration,
    ) -> Result<EngramTargetReconciliationReceipt, EngramError> {
        validate_target_pair(left, right)?;
        recover_target_stage_files(left_filesystem, &left.root)?;
        recover_target_stage_files(right_filesystem, &right.root)?;
        let left_manifest = read_target_manifest(left_filesystem, &left.root)?;
        let right_manifest = read_target_manifest(right_filesystem, &right.root)?;
        let left_chunks = manifest_map(&left_manifest)?;
        let right_chunks = manifest_map(&right_manifest)?;
        let mut moved = Vec::new();
        for id in left_chunks
            .keys()
            .filter(|id| !right_chunks.contains_key(*id))
        {
            moved.push(copy_target_chunk(
                left_filesystem,
                left,
                right_filesystem,
                right,
                id,
            )?);
        }
        for id in right_chunks
            .keys()
            .filter(|id| !left_chunks.contains_key(*id))
        {
            moved.push(copy_target_chunk(
                right_filesystem,
                right,
                left_filesystem,
                left,
                id,
            )?);
        }
        for id in left_chunks
            .keys()
            .filter(|id| right_chunks.contains_key(*id))
        {
            let left_bytes = read_target_chunk(left_filesystem, &left.root, id)?;
            let right_bytes = read_target_chunk(right_filesystem, &right.root, id)?;
            if digest_chunk_bytes(id, &left_bytes)?.digest
                != digest_chunk_bytes(id, &right_bytes)?.digest
            {
                return Err(EngramError::ChunkCollision(id.clone()));
            }
        }
        let merged = merge_manifests(left_manifest, right_manifest)?;
        write_target_manifest(left_filesystem, &left.root, &merged)?;
        write_target_manifest(right_filesystem, &right.root, &merged)?;
        Ok(EngramTargetReconciliationReceipt {
            project_id: left.project_id.clone(),
            moved,
            unresolved: None,
        })
    }

    pub fn reconcile_granted_targets(
        _left_filesystem: &dyn TargetFilesystem,
        left: &EngramTargetChunkSetDeclaration,
        _right_filesystem: &dyn TargetFilesystem,
        right: &EngramTargetChunkSetDeclaration,
        grant: &EngramGrant,
    ) -> Result<EngramTargetReconciliationReceipt, EngramError> {
        if grant.project_id != left.project_id
            || grant.project_id != right.project_id
            || grant.grantor != left.owner_id
            || grant.grantee != right.owner_id
        {
            return Err(EngramError::GrantMismatch);
        }
        if grant.state == EngramGrantState::Withdrawn {
            return Ok(EngramTargetReconciliationReceipt {
                project_id: left.project_id.clone(),
                moved: Vec::new(),
                unresolved: Some(
                    "grant withdrawn; previously materialized observations are retained".to_owned(),
                ),
            });
        }
        if left.scope != EngramScope::Project || right.scope != EngramScope::Project {
            return Err(EngramError::PersonalTransport);
        }
        // The v1 JSON file is metadata only. Until Engram provides an
        // authenticated upstream attestation, accepting it would let a peer
        // self-classify arbitrary mixed-scope chunks as project data.
        Err(EngramError::MissingScopeAttestation)
    }

    pub fn reconcile_active_targets<L, R>(
        left_filesystem: &L,
        left: &EngramTargetChunkSetDeclaration,
        right_filesystem: &R,
        right: &EngramTargetChunkSetDeclaration,
    ) -> Result<EngramTargetReconciliationReceipt, EngramError>
    where
        L: TargetFilesystem + EngramTargetRuntime,
        R: TargetFilesystem + EngramTargetRuntime,
    {
        for result in [
            verify_runtime_owner(left_filesystem, left),
            verify_runtime_owner(right_filesystem, right),
        ] {
            if let Err(error) = result {
                if let EngramError::Target(target) = &error
                    && target_error_is_unreachable(target)
                {
                    return Ok(EngramTargetReconciliationReceipt {
                        project_id: left.project_id.clone(),
                        moved: Vec::new(),
                        unresolved: Some("target is unreachable".to_owned()),
                    });
                }
                return Err(error);
            }
        }
        for result in [
            left_filesystem
                .sync_engram(
                    &left.project_id,
                    &left.project_root,
                    EngramTargetSyncMode::Export,
                )
                .map_err(EngramError::Target),
            right_filesystem
                .sync_engram(
                    &right.project_id,
                    &right.project_root,
                    EngramTargetSyncMode::Export,
                )
                .map_err(EngramError::Target),
        ] {
            if let Err(error) = result {
                if let EngramError::Target(target) = &error
                    && target_error_is_unreachable(target)
                {
                    return Ok(EngramTargetReconciliationReceipt {
                        project_id: left.project_id.clone(),
                        moved: Vec::new(),
                        unresolved: Some("target is unreachable".to_owned()),
                    });
                }
                return Err(error);
            }
        }
        let receipt = match Self::reconcile_targets(left_filesystem, left, right_filesystem, right)
        {
            Ok(receipt) => receipt,
            Err(EngramError::Target(target)) if target_error_is_unreachable(&target) => {
                EngramTargetReconciliationReceipt {
                    project_id: left.project_id.clone(),
                    moved: Vec::new(),
                    unresolved: Some("target is unreachable".to_owned()),
                }
            }
            Err(error) => return Err(error),
        };
        if receipt.unresolved.is_some() {
            return Ok(receipt);
        }
        for result in [
            left_filesystem
                .sync_engram(
                    &left.project_id,
                    &left.project_root,
                    EngramTargetSyncMode::Import,
                )
                .map_err(EngramError::Target),
            right_filesystem
                .sync_engram(
                    &right.project_id,
                    &right.project_root,
                    EngramTargetSyncMode::Import,
                )
                .map_err(EngramError::Target),
        ] {
            if let Err(error) = result {
                if let EngramError::Target(target) = &error
                    && target_error_is_unreachable(target)
                {
                    return Ok(EngramTargetReconciliationReceipt {
                        project_id: left.project_id.clone(),
                        moved: Vec::new(),
                        unresolved: Some("target is unreachable".to_owned()),
                    });
                }
                return Err(error);
            }
        }
        Ok(receipt)
    }
}

fn verify_runtime_owner<T: EngramTargetRuntime>(
    runtime: &T,
    declaration: &EngramTargetChunkSetDeclaration,
) -> Result<(), EngramError> {
    let principal = runtime.resolve_principal()?;
    let owner = EngramOwnerId::from_principal(&principal);
    if owner != declaration.owner_id {
        return Err(EngramError::OwnerBindingMismatch);
    }
    Ok(())
}

fn target_error_is_unreachable(error: &TargetFilesystemError) -> bool {
    match error {
        TargetFilesystemError::RemoteFailure { status: 255, .. } => true,
        TargetFilesystemError::Io(error) => matches!(
            error.kind(),
            std::io::ErrorKind::ConnectionRefused
                | std::io::ErrorKind::ConnectionReset
                | std::io::ErrorKind::ConnectionAborted
                | std::io::ErrorKind::NotConnected
                | std::io::ErrorKind::TimedOut
                | std::io::ErrorKind::NetworkUnreachable
                | std::io::ErrorKind::HostUnreachable
        ),
        _ => false,
    }
}

fn validate_target_pair(
    left: &EngramTargetChunkSetDeclaration,
    right: &EngramTargetChunkSetDeclaration,
) -> Result<(), EngramError> {
    if left.scope == EngramScope::Personal || right.scope == EngramScope::Personal {
        return Err(EngramError::PersonalTransport);
    }
    if left.project_id != right.project_id {
        return Err(EngramError::ProjectIdentityMismatch);
    }
    if left.owner_id != right.owner_id {
        return Err(EngramError::MissingScopeAttestation);
    }
    Ok(())
}

fn target_path(
    root: &NormalizedManagedPath,
    suffix: &str,
) -> Result<NormalizedManagedPath, EngramError> {
    NormalizedManagedPath::parse(format!("{}/{suffix}", root.as_str()))
        .map_err(EngramError::Resource)
}

fn read_target_manifest(
    filesystem: &dyn TargetFilesystem,
    root: &NormalizedManagedPath,
) -> Result<EngramManifest, EngramError> {
    let bytes = filesystem
        .read_file(&target_path(root, "manifest.json")?)?
        .ok_or(EngramError::TargetUnreachable)?;
    let manifest: EngramManifest = serde_json::from_slice(&bytes)?;
    if manifest.version != 1 {
        return Err(EngramError::UnsupportedManifestVersion);
    }
    manifest_map(&manifest)?;
    Ok(manifest)
}

fn observed_target_chunk_ids(
    filesystem: &dyn TargetFilesystem,
    root: &NormalizedManagedPath,
) -> Result<BTreeSet<String>, EngramError> {
    let names = filesystem.list_directory(&target_path(root, "chunks")?)?;
    let mut ids = BTreeSet::new();
    for name in names {
        if name.ends_with(".jsonl.gz.commonkit-tmp") {
            continue;
        }
        let id = name
            .strip_suffix(".jsonl.gz")
            .ok_or_else(|| EngramError::InvalidChunkId(name.clone()))?;
        validate_chunk_id(id)?;
        ids.insert(id.to_owned());
    }
    Ok(ids)
}

fn recover_target_stage_files(
    filesystem: &dyn TargetFilesystem,
    root: &NormalizedManagedPath,
) -> Result<(), EngramError> {
    let manifest_temporary = target_path(root, ".manifest.json.commonkit-tmp")?;
    match filesystem.remove(&manifest_temporary) {
        Ok(()) | Err(TargetFilesystemError::ReadOnly) => {}
        Err(error) => return Err(error.into()),
    }
    let chunks = target_path(root, "chunks")?;
    for name in filesystem.list_directory(&chunks)? {
        if !name.starts_with('.') || !name.ends_with(".jsonl.gz.commonkit-tmp") {
            continue;
        }
        let temporary = target_path(root, &format!("chunks/{name}"))?;
        match filesystem.remove(&temporary) {
            Ok(()) | Err(TargetFilesystemError::ReadOnly) => {}
            Err(error) => return Err(error.into()),
        }
    }
    Ok(())
}

fn read_target_chunk(
    filesystem: &dyn TargetFilesystem,
    root: &NormalizedManagedPath,
    id: &str,
) -> Result<Vec<u8>, EngramError> {
    validate_chunk_id(id)?;
    let bytes = filesystem
        .read_file(&target_path(root, &format!("chunks/{id}.jsonl.gz"))?)?
        .ok_or_else(|| EngramError::MissingChunk(id.to_owned()))?;
    if bytes.len() as u64 > MAX_ENGRAM_CHUNK_BYTES {
        return Err(EngramError::ChunkTooLarge);
    }
    Ok(bytes)
}

fn digest_chunk_bytes(id: &str, bytes: &[u8]) -> Result<EngramChunkDigest, EngramError> {
    Ok(EngramChunkDigest {
        id: id.to_owned(),
        digest: Sha256Digest::parse(format!("sha256:{:x}", Sha256::digest(bytes)))?,
        bytes: u64::try_from(bytes.len()).map_err(|_| EngramError::ChunkTooLarge)?,
    })
}

fn copy_target_chunk(
    from_filesystem: &dyn TargetFilesystem,
    from: &EngramTargetChunkSetDeclaration,
    to_filesystem: &dyn TargetFilesystem,
    to: &EngramTargetChunkSetDeclaration,
    id: &str,
) -> Result<EngramTargetChunkMovement, EngramError> {
    let bytes = read_target_chunk(from_filesystem, &from.root, id)?;
    let digest = digest_chunk_bytes(id, &bytes)?;
    let destination = target_path(&to.root, &format!("chunks/{id}.jsonl.gz"))?;
    to_filesystem.write_file(&destination, &bytes)?;
    let observed = read_target_chunk(to_filesystem, &to.root, id)?;
    if digest_chunk_bytes(id, &observed)? != digest {
        return Err(EngramError::ChunkDigestMismatch(id.to_owned()));
    }
    Ok(EngramTargetChunkMovement {
        chunk_id: id.to_owned(),
        digest: digest.digest,
        bytes: digest.bytes,
        from_target: from.target_id.clone(),
        to_target: to.target_id.clone(),
    })
}

fn write_target_manifest(
    filesystem: &dyn TargetFilesystem,
    root: &NormalizedManagedPath,
    manifest: &EngramManifest,
) -> Result<(), EngramError> {
    let mut bytes = serde_json::to_vec_pretty(manifest)?;
    bytes.push(b'\n');
    filesystem.write_file(&target_path(root, "manifest.json")?, &bytes)?;
    Ok(())
}

fn run_engram(
    runner: &dyn EngramCommandRunner,
    working_directory: &Path,
    arguments: &[&str],
) -> Result<(), EngramError> {
    let output = runner.run(working_directory, arguments)?;
    if output.success {
        Ok(())
    } else {
        Err(EngramError::CommandFailed(arguments.join(" ")))
    }
}

fn manifest_map(
    manifest: &EngramManifest,
) -> Result<BTreeMap<String, EngramManifestChunk>, EngramError> {
    let mut chunks = BTreeMap::new();
    for chunk in &manifest.chunks {
        validate_chunk_id(&chunk.id)?;
        if chunks.insert(chunk.id.clone(), chunk.clone()).is_some() {
            return Err(EngramError::DuplicateChunk(chunk.id.clone()));
        }
    }
    Ok(chunks)
}

fn merge_manifests(
    left: EngramManifest,
    right: EngramManifest,
) -> Result<EngramManifest, EngramError> {
    if left.version != 1 || right.version != 1 {
        return Err(EngramError::UnsupportedManifestVersion);
    }
    let mut chunks = manifest_map(&left)?;
    for (id, chunk) in manifest_map(&right)? {
        if let Some(existing) = chunks.get(&id) {
            if serde_json::to_value(existing)? != serde_json::to_value(&chunk)? {
                return Err(EngramError::ManifestCollision(id));
            }
        } else {
            chunks.insert(id, chunk);
        }
    }
    Ok(EngramManifest {
        version: 1,
        chunks: chunks.into_values().collect(),
    })
}

fn read_manifest(root: &Path) -> Result<EngramManifest, EngramError> {
    let root_dir = open_engram_root(root)?;
    let mut file = crate::target::open_target_file_nofollow(&root_dir, Path::new("manifest.json"))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    let manifest: EngramManifest = serde_json::from_slice(&bytes)?;
    if manifest.version != 1 {
        return Err(EngramError::UnsupportedManifestVersion);
    }
    manifest_map(&manifest)?;
    Ok(manifest)
}

fn observed_chunk_ids(root: &Path) -> Result<BTreeSet<String>, EngramError> {
    let root_dir = open_engram_root(root)?;
    let chunks = crate::target::open_target_dir_nofollow(&root_dir, Path::new("chunks"))?;
    let mut ids = BTreeSet::new();
    for entry in chunks.entries()? {
        let entry = entry?;
        let metadata = entry.metadata()?;
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(EngramError::UnsafePath(
                root.join("chunks").join(entry.file_name()),
            ));
        }
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| EngramError::UnsafePath(root.join("chunks").join(entry.file_name())))?;
        let id = name
            .strip_suffix(".jsonl.gz")
            .ok_or_else(|| EngramError::UnsafePath(root.join("chunks").join(entry.file_name())))?;
        validate_chunk_id(id)?;
        ids.insert(id.to_owned());
    }
    Ok(ids)
}

fn validate_chunk_id(id: &str) -> Result<(), EngramError> {
    if id.len() == 8
        && id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        Ok(())
    } else {
        Err(EngramError::InvalidChunkId(id.to_owned()))
    }
}

fn chunk_path(root: &Path, id: &str) -> PathBuf {
    root.join("chunks").join(format!("{id}.jsonl.gz"))
}

fn chunk_digest(root: &Path, id: &str) -> Result<EngramChunkDigest, EngramError> {
    validate_chunk_id(id)?;
    let root_dir = open_engram_root(root)?;
    let chunks = crate::target::open_target_dir_nofollow(&root_dir, Path::new("chunks"))?;
    let file =
        crate::target::open_target_file_nofollow(&chunks, Path::new(&format!("{id}.jsonl.gz")))?;
    digest_open_chunk(id, file)
}

fn digest_open_chunk(
    id: &str,
    mut file: cap_std::fs::File,
) -> Result<EngramChunkDigest, EngramError> {
    let bytes = file.metadata()?.len();
    if bytes > MAX_ENGRAM_CHUNK_BYTES {
        return Err(EngramError::ChunkTooLarge);
    }
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    let digest = Sha256Digest::parse(format!("sha256:{:x}", hasher.finalize()))?;
    Ok(EngramChunkDigest {
        id: id.to_owned(),
        digest,
        bytes,
    })
}

fn copy_chunk(from: &Path, to: &Path, id: &str) -> Result<EngramChunkMovement, EngramError> {
    let digest = chunk_digest(from, id)?;
    let from_root = open_engram_root(from)?;
    let to_root = open_engram_root(to)?;
    let from_chunks = crate::target::open_target_dir_nofollow(&from_root, Path::new("chunks"))?;
    let to_chunks = crate::target::open_target_dir_nofollow(&to_root, Path::new("chunks"))?;
    let name = PathBuf::from(format!("{id}.jsonl.gz"));
    if let Ok(metadata) = to_chunks.symlink_metadata(&name) {
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(EngramError::UnsafePath(chunk_path(to, id)));
        }
        let copied = digest_open_chunk(
            id,
            crate::target::open_target_file_nofollow(&to_chunks, &name)?,
        )?;
        if copied.digest != digest.digest || copied.bytes != digest.bytes {
            return Err(EngramError::ChunkDigestMismatch(id.to_owned()));
        }
        return Ok(EngramChunkMovement {
            chunk_id: id.to_owned(),
            digest: digest.digest,
            bytes: digest.bytes,
            from: from.to_path_buf(),
            to: to.to_path_buf(),
        });
    }
    let mut input = crate::target::open_target_file_nofollow(&from_chunks, &name)?;
    let temporary = PathBuf::from(format!(".{id}.jsonl.gz.commonkit-tmp"));
    let mut options = cap_std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut output = to_chunks.open_with(&temporary, &options)?;
    std::io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    to_chunks.rename(&temporary, &to_chunks, &name)?;
    sync_directory(to)?;
    let copied = digest_open_chunk(
        id,
        crate::target::open_target_file_nofollow(&to_chunks, &name)?,
    )?;
    if copied.digest != digest.digest || copied.bytes != digest.bytes {
        return Err(EngramError::ChunkDigestMismatch(id.to_owned()));
    }
    Ok(EngramChunkMovement {
        chunk_id: id.to_owned(),
        digest: digest.digest,
        bytes: digest.bytes,
        from: from.to_path_buf(),
        to: to.to_path_buf(),
    })
}

fn write_manifest(root: &Path, manifest: &EngramManifest) -> Result<(), EngramError> {
    let root_dir = open_engram_root(root)?;
    let path = PathBuf::from("manifest.json");
    let temporary = PathBuf::from(".manifest.json.commonkit-tmp");
    let _ = root_dir.remove_file(&temporary);
    let bytes = serde_json::to_vec_pretty(manifest)?;
    let mut options = cap_std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    let mut output = root_dir.open_with(&temporary, &options)?;
    output.write_all(&bytes)?;
    output.write_all(b"\n")?;
    output.sync_all()?;
    root_dir.rename(&temporary, &root_dir, &path)?;
    sync_directory(root)?;
    Ok(())
}

fn recover_local_stage_files(root: &Path) -> Result<(), EngramError> {
    let root_dir = open_engram_root(root)?;
    let _ = root_dir.remove_file(Path::new(".manifest.json.commonkit-tmp"));
    let _ = root_dir.remove_file(Path::new("manifest.json.commonkit-tmp"));
    let chunks = crate::target::open_target_dir_nofollow(&root_dir, Path::new("chunks"))?;
    for entry in chunks.entries()? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(EngramError::UnsafePath(
                root.join("chunks").join(entry.file_name()),
            ));
        };
        if name.ends_with(".jsonl.gz.commonkit-tmp") {
            chunks.remove_file(Path::new(name))?;
        }
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), EngramError> {
    open_engram_root(path)?
        .try_clone()?
        .into_std_file()
        .sync_all()?;
    Ok(())
}

#[derive(Debug, Error)]
pub enum EngramError {
    #[error("invalid Engram project identity: {0}")]
    InvalidProjectId(String),
    #[error("invalid Engram owner identity: {0}")]
    InvalidOwnerId(String),
    #[error("invalid Engram chunk ID: {0}")]
    InvalidChunkId(String),
    #[error("duplicate Engram chunk in manifest: {0}")]
    DuplicateChunk(String),
    #[error("unsupported Engram manifest version")]
    UnsupportedManifestVersion,
    #[error("personal Engram chunks are never transportable")]
    PersonalTransport,
    #[error("Engram project identities do not match")]
    ProjectIdentityMismatch,
    #[error("cross-principal Engram transport requires project-scope attestation")]
    MissingScopeAttestation,
    #[error("Engram target principal does not match the declared owner")]
    OwnerBindingMismatch,
    #[error("Engram project-scope attestation does not match the exported chunks")]
    InvalidScopeAttestation,
    #[error("Engram Grant does not authorize these principals and project")]
    GrantMismatch,
    #[error("Engram chunk ID {0} has conflicting payloads")]
    ChunkCollision(String),
    #[error("Engram chunk ID {0} has conflicting manifest metadata")]
    ManifestCollision(String),
    #[error("Engram chunk ID {0} changed while being copied")]
    ChunkDigestMismatch(String),
    #[error("Engram chunk is missing: {0}")]
    MissingChunk(String),
    #[error("Engram chunk is too large")]
    ChunkTooLarge,
    #[error("Engram target is unreachable")]
    TargetUnreachable,
    #[error("Engram chunk root has no project directory")]
    MissingProjectRoot,
    #[error("engram command failed: {0}")]
    CommandFailed(String),
    #[error("unsafe Engram path: {0}")]
    UnsafePath(PathBuf),
    #[error(transparent)]
    Contract(#[from] commonkit_contracts::ContractError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Resource(#[from] crate::ResourceError),
    #[error(transparent)]
    Target(#[from] TargetFilesystemError),
}
