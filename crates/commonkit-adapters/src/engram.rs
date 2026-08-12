use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::Command;

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
}

impl ProcessEngramCommandRunner {
    pub fn from_path(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
        }
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
        let status = Command::new(&self.executable)
            .args(arguments)
            .current_dir(working_directory)
            .status()?;
        Ok(EngramCommandOutput {
            success: status.success(),
        })
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
        left_filesystem: &dyn TargetFilesystem,
        left: &EngramTargetChunkSetDeclaration,
        right_filesystem: &dyn TargetFilesystem,
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
        let left_attestation = verify_target_attestation(left_filesystem, left)?;
        let right_attestation = verify_target_attestation(right_filesystem, right)?;
        let mut same_owner_right = right.clone();
        same_owner_right.owner_id = left.owner_id.clone();
        let receipt =
            Self::reconcile_targets(left_filesystem, left, right_filesystem, &same_owner_right)?;
        let merged_manifest = left_filesystem
            .read_file(&target_path(&left.root, "manifest.json")?)?
            .ok_or(EngramError::TargetUnreachable)?;
        let merged = merge_attestations(left_attestation, right_attestation, &merged_manifest)?;
        write_target_attestation(left_filesystem, &left.root, &merged)?;
        write_target_attestation(right_filesystem, &right.root, &merged)?;
        Ok(receipt)
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
        if left_filesystem
            .sync_engram(
                &left.project_id,
                &left.project_root,
                EngramTargetSyncMode::Export,
            )
            .is_err()
        {
            return Ok(EngramTargetReconciliationReceipt {
                project_id: left.project_id.clone(),
                moved: Vec::new(),
                unresolved: Some("target is unreachable".to_owned()),
            });
        }
        if right_filesystem
            .sync_engram(
                &right.project_id,
                &right.project_root,
                EngramTargetSyncMode::Export,
            )
            .is_err()
        {
            return Ok(EngramTargetReconciliationReceipt {
                project_id: left.project_id.clone(),
                moved: Vec::new(),
                unresolved: Some("target is unreachable".to_owned()),
            });
        }
        let receipt = match Self::reconcile_targets(left_filesystem, left, right_filesystem, right)
        {
            Ok(receipt) => receipt,
            Err(EngramError::Target(_)) => EngramTargetReconciliationReceipt {
                project_id: left.project_id.clone(),
                moved: Vec::new(),
                unresolved: Some("target is unreachable".to_owned()),
            },
            Err(error) => return Err(error),
        };
        if receipt.unresolved.is_some() {
            return Ok(receipt);
        }
        if left_filesystem
            .sync_engram(
                &left.project_id,
                &left.project_root,
                EngramTargetSyncMode::Import,
            )
            .is_err()
        {
            return Ok(EngramTargetReconciliationReceipt {
                project_id: left.project_id.clone(),
                moved: Vec::new(),
                unresolved: Some("target is unreachable".to_owned()),
            });
        }
        if right_filesystem
            .sync_engram(
                &right.project_id,
                &right.project_root,
                EngramTargetSyncMode::Import,
            )
            .is_err()
        {
            return Ok(EngramTargetReconciliationReceipt {
                project_id: left.project_id.clone(),
                moved: Vec::new(),
                unresolved: Some("target is unreachable".to_owned()),
            });
        }
        Ok(receipt)
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

fn verify_target_attestation(
    filesystem: &dyn TargetFilesystem,
    declaration: &EngramTargetChunkSetDeclaration,
) -> Result<EngramScopeAttestation, EngramError> {
    let bytes = filesystem
        .read_file(&target_path(&declaration.root, "scope-attestation.json")?)?
        .ok_or(EngramError::MissingScopeAttestation)?;
    let attestation: EngramScopeAttestation = serde_json::from_slice(&bytes)?;
    let manifest_bytes = filesystem
        .read_file(&target_path(&declaration.root, "manifest.json")?)?
        .ok_or(EngramError::TargetUnreachable)?;
    let manifest_digest =
        Sha256Digest::parse(format!("sha256:{:x}", Sha256::digest(&manifest_bytes)))?;
    if attestation.schema != "engram.scope-export.v1"
        || attestation.exporter_version.trim().is_empty()
        || attestation.project_id != declaration.project_id
        || attestation.scope != EngramScope::Project
        || attestation.manifest_digest != manifest_digest
    {
        return Err(EngramError::InvalidScopeAttestation);
    }
    let manifest = read_target_manifest(filesystem, &declaration.root)?;
    let manifest_ids = manifest_map(&manifest)?
        .into_keys()
        .collect::<BTreeSet<_>>();
    let mut attested = BTreeMap::new();
    for chunk in &attestation.chunks {
        validate_chunk_id(&chunk.id)?;
        if attested.insert(chunk.id.clone(), chunk.clone()).is_some() {
            return Err(EngramError::InvalidScopeAttestation);
        }
    }
    if manifest_ids != attested.keys().cloned().collect() {
        return Err(EngramError::InvalidScopeAttestation);
    }
    for id in manifest_ids {
        let observed =
            digest_chunk_bytes(&id, &read_target_chunk(filesystem, &declaration.root, &id)?)?;
        if attested.get(&id) != Some(&observed) {
            return Err(EngramError::InvalidScopeAttestation);
        }
    }
    Ok(attestation)
}

fn merge_attestations(
    left: EngramScopeAttestation,
    right: EngramScopeAttestation,
    merged_manifest: &[u8],
) -> Result<EngramScopeAttestation, EngramError> {
    if left.schema != right.schema
        || left.exporter_version != right.exporter_version
        || left.project_id != right.project_id
        || left.scope != right.scope
    {
        return Err(EngramError::InvalidScopeAttestation);
    }
    let mut chunks = BTreeMap::new();
    for chunk in left.chunks.into_iter().chain(right.chunks) {
        if let Some(existing) = chunks.insert(chunk.id.clone(), chunk.clone())
            && existing != chunk
        {
            return Err(EngramError::InvalidScopeAttestation);
        }
    }
    Ok(EngramScopeAttestation {
        schema: left.schema,
        exporter_version: left.exporter_version,
        project_id: left.project_id,
        scope: left.scope,
        manifest_digest: Sha256Digest::parse(format!(
            "sha256:{:x}",
            Sha256::digest(merged_manifest)
        ))?,
        chunks: chunks.into_values().collect(),
    })
}

fn write_target_attestation(
    filesystem: &dyn TargetFilesystem,
    root: &NormalizedManagedPath,
    attestation: &EngramScopeAttestation,
) -> Result<(), EngramError> {
    let mut bytes = serde_json::to_vec_pretty(attestation)?;
    bytes.push(b'\n');
    filesystem.write_file(&target_path(root, "scope-attestation.json")?, &bytes)?;
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
        let id = name
            .strip_suffix(".jsonl.gz")
            .ok_or_else(|| EngramError::InvalidChunkId(name.clone()))?;
        validate_chunk_id(id)?;
        ids.insert(id.to_owned());
    }
    Ok(ids)
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
    let metadata = fs::symlink_metadata(root.join("manifest.json"))?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(EngramError::UnsafePath(root.join("manifest.json")));
    }
    let manifest: EngramManifest = serde_json::from_slice(&fs::read(root.join("manifest.json"))?)?;
    if manifest.version != 1 {
        return Err(EngramError::UnsupportedManifestVersion);
    }
    manifest_map(&manifest)?;
    Ok(manifest)
}

fn observed_chunk_ids(root: &Path) -> Result<BTreeSet<String>, EngramError> {
    let chunks = root.join("chunks");
    let metadata = fs::symlink_metadata(&chunks)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(EngramError::UnsafePath(chunks));
    }
    let mut ids = BTreeSet::new();
    for entry in fs::read_dir(chunks)? {
        let entry = entry?;
        let metadata = entry.file_type()?;
        if !metadata.is_file() || metadata.is_symlink() {
            return Err(EngramError::UnsafePath(entry.path()));
        }
        let name = entry.file_name();
        let name = name
            .to_str()
            .ok_or_else(|| EngramError::UnsafePath(entry.path()))?;
        let id = name
            .strip_suffix(".jsonl.gz")
            .ok_or_else(|| EngramError::UnsafePath(entry.path()))?;
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
    let path = chunk_path(root, id);
    let metadata = fs::symlink_metadata(&path)?;
    if !metadata.is_file() || metadata.file_type().is_symlink() {
        return Err(EngramError::UnsafePath(path));
    }
    if metadata.len() > MAX_ENGRAM_CHUNK_BYTES {
        return Err(EngramError::ChunkTooLarge);
    }
    let mut file = OpenOptions::new().read(true).open(&path)?;
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
        bytes: metadata.len(),
    })
}

fn copy_chunk(from: &Path, to: &Path, id: &str) -> Result<EngramChunkMovement, EngramError> {
    let source = chunk_path(from, id);
    let destination = chunk_path(to, id);
    let digest = chunk_digest(from, id)?;
    let temporary = destination.with_extension("jsonl.gz.commonkit-tmp");
    if let Ok(metadata) = fs::symlink_metadata(&destination) {
        if !metadata.is_file() || metadata.file_type().is_symlink() {
            return Err(EngramError::UnsafePath(destination));
        }
        let copied = chunk_digest(to, id)?;
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
    let mut input = OpenOptions::new().read(true).open(&source)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    std::io::copy(&mut input, &mut output)?;
    output.sync_all()?;
    fs::rename(&temporary, &destination)?;
    sync_directory(to)?;
    let copied = chunk_digest(to, id)?;
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
    let path = root.join("manifest.json");
    let temporary = root.join("manifest.json.commonkit-tmp");
    remove_stale_file(&temporary)?;
    let bytes = serde_json::to_vec_pretty(manifest)?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    output.write_all(&bytes)?;
    output.write_all(b"\n")?;
    output.sync_all()?;
    fs::rename(temporary, path)?;
    sync_directory(root)?;
    Ok(())
}

fn recover_local_stage_files(root: &Path) -> Result<(), EngramError> {
    remove_stale_file(&root.join("manifest.json.commonkit-tmp"))?;
    let chunks = root.join("chunks");
    let metadata = fs::symlink_metadata(&chunks)?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err(EngramError::UnsafePath(chunks));
    }
    for entry in fs::read_dir(&chunks)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            return Err(EngramError::UnsafePath(path));
        };
        if name.ends_with(".jsonl.gz.commonkit-tmp") {
            remove_stale_file(&path)?;
        }
    }
    Ok(())
}

fn remove_stale_file(path: &Path) -> Result<(), EngramError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() && !metadata.file_type().is_symlink() => {
            fs::remove_file(path)?;
        }
        Ok(_) => return Err(EngramError::UnsafePath(path.to_path_buf())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

fn sync_directory(path: &Path) -> Result<(), EngramError> {
    fs::File::open(path)?.sync_all()?;
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
