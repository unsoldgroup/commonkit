//! Snapshot orchestration for mutable databases.
//!
//! Portable metadata contains digests and encrypted-object references, never plaintext data.

use chacha20poly1305::aead::{Aead, KeyInit, Payload};
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fmt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Mutex;
use std::time::Duration;
use zeroize::Zeroize;

#[derive(Clone, Debug, Eq, PartialEq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct DatabaseId(String);

impl DatabaseId {
    pub fn new(value: impl Into<String>) -> Result<Self, SnapshotError> {
        let value = value.into();
        if value.is_empty()
            || !value
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        {
            return Err(SnapshotError::InvalidDatabaseId);
        }
        Ok(Self(value))
    }
}

impl fmt::Display for DatabaseId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "state", rename_all = "camelCase")]
pub enum Authority {
    Unassigned,
    Writer { target: String },
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SnapshotCoordinator {
    database: DatabaseId,
    authority: Authority,
    latest_snapshot_digest: Option<String>,
}

impl SnapshotCoordinator {
    pub fn new(database: DatabaseId) -> Self {
        Self {
            database,
            authority: Authority::Unassigned,
            latest_snapshot_digest: None,
        }
    }

    pub fn authority(&self) -> &Authority {
        &self.authority
    }

    pub fn initialize_writer(&mut self, target: impl Into<String>) -> Result<(), SnapshotError> {
        match &self.authority {
            Authority::Unassigned => {
                self.authority = Authority::Writer {
                    target: target.into(),
                };
                Ok(())
            }
            Authority::Writer { target } => {
                Err(SnapshotError::WriterAlreadyAssigned(target.clone()))
            }
        }
    }

    pub fn record_snapshot_digest(&mut self, digest: impl Into<String>) {
        self.latest_snapshot_digest = Some(digest.into());
    }

    pub fn promote(
        &mut self,
        candidate: impl Into<String>,
        current_writer_digest: &str,
        candidate_digest: &str,
    ) -> Result<(), SnapshotError> {
        let latest = self
            .latest_snapshot_digest
            .as_deref()
            .ok_or(SnapshotError::NoSnapshot)?;
        if current_writer_digest != latest {
            return Err(SnapshotError::UnsnapshottedWriterChanges);
        }
        if candidate_digest != latest {
            return Err(SnapshotError::CandidateDoesNotMatchSnapshot);
        }
        self.authority = Authority::Writer {
            target: candidate.into(),
        };
        Ok(())
    }
}

#[derive(Clone, Debug, Eq, PartialEq, thiserror::Error)]
pub enum SnapshotError {
    #[error("database id must contain only ASCII letters, digits, '-' or '_'")]
    InvalidDatabaseId,
    #[error("authoritative writer is already assigned to {0}")]
    WriterAlreadyAssigned(String),
    #[error("snapshot object was not found: {0}")]
    ObjectNotFound(String),
    #[error("snapshot ciphertext authentication failed")]
    AuthenticationFailed,
    #[error("snapshot content digest does not match its manifest")]
    IntegrityMismatch,
    #[error("restored database failed post-swap verification; prior database was restored")]
    RestoreVerificationFailed,
    #[error("no snapshot exists for promotion")]
    NoSnapshot,
    #[error("authoritative writer has changes newer than the latest snapshot")]
    UnsnapshottedWriterChanges,
    #[error("promotion candidate does not match the latest snapshot")]
    CandidateDoesNotMatchSnapshot,
    #[error("database backup or integrity validation failed")]
    DatabaseFailed,
    #[error("snapshot object-store operation failed")]
    ObjectStoreFailed,
    #[error("snapshot object-store configuration is invalid")]
    InvalidObjectStore,
    #[error("unsupported snapshot schema or cipher")]
    UnsupportedSnapshotFormat,
    #[error("durable snapshot transaction metadata failed integrity validation")]
    TransactionIntegrity,
    #[error("snapshot transaction was interrupted and requires recovery")]
    Interrupted,
    #[error("snapshot service lifecycle operation failed")]
    LifecycleFailed,
    #[error("portable snapshot authority changed since it was reviewed")]
    StalePortableAuthority,
    #[error("portable snapshot authority references missing or rolled-back history")]
    PortableAuthorityRollback,
}

/// Lifecycle coordination around database replacement. Implementations must not return from
/// `stop` until writers have quiesced or from `start` until the service can reopen the database.
pub trait DatabaseLifecycle {
    fn stop(&mut self) -> Result<(), SnapshotError>;
    fn start(&mut self) -> Result<(), SnapshotError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RestoreFailpoint {
    None,
    AfterSwap,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestorePlan {
    pub schema: String,
    pub run_id: String,
    pub snapshot_id: String,
    pub manifest_digest: String,
    pub database: DatabaseId,
    pub expected_content_digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RestoreReceipt {
    pub schema: String,
    pub run_id: String,
    pub plan_digest: String,
    pub state: RestoreState,
    pub preimage_object_digest: Option<String>,
    pub resulting_content_digest: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromotionPlan {
    pub schema: String,
    pub run_id: String,
    pub database: DatabaseId,
    pub previous_writer: String,
    pub candidate_writer: String,
    pub latest_snapshot_digest: String,
    pub current_writer_digest: String,
    pub candidate_digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PromotionReceipt {
    pub schema: String,
    pub run_id: String,
    pub plan_digest: String,
    pub database: DatabaseId,
    pub previous_writer: String,
    pub authoritative_writer: String,
    pub snapshot_digest: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PromotionRecovery {
    Committed(PromotionReceipt),
    Aborted,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
enum PromotionPhase {
    Prepared,
    CasConfirmed { portable_revision: String },
    Aborted { portable_revision: String },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PromotionFailpoint {
    None,
    AfterAuthorityWrite,
}

/// Durable authoritative-writer state. Promotion validates both sides against the latest
/// snapshot and atomically records an integrity-checked plan, writer state, and receipt.
pub struct AuthorityStore {
    root: PathBuf,
}

/// Portable, authenticated source of truth for a mutable database's single writer and accepted
/// snapshot head. Each transition is also retained in authenticated immutable history, which
/// detects replay of an older mutable pointer when newer history is present. The `revision`
/// returned alongside a record is the compare-and-swap token that a Git integration must bind to
/// the repository revision it fetched. A push must use the fetched Git commit as its expected
/// parent; a non-fast-forward push protects against rollback of the complete portable tree.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PortableAuthorityRecord {
    pub schema: String,
    pub database: DatabaseId,
    pub current_writer: String,
    pub accepted_head: Option<String>,
    pub accepted_descriptor: Option<String>,
    pub generation: u64,
    pub previous_revision: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct VersionedPortableAuthority {
    pub record: PortableAuthorityRecord,
    pub revision: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PublishedPortableAuthority {
    pub authority: VersionedPortableAuthority,
    pub repository_revision: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PortablePublication {
    pub database: DatabaseId,
    pub generation: u64,
    pub authority_revision: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecoveredPortablePublication {
    pub database: DatabaseId,
    pub generation: u64,
    pub authority_revision: String,
    pub repository_revision: String,
}

/// Atomically publishes a staged portable tree only if `expected_parent` is still the trusted
/// remote branch head. Implementations must use a non-force, fast-forward-only remote update.
pub trait PortableAuthorityPublisher {
    fn publish(
        &mut self,
        expected_parent: &str,
        staged_portable_root: &Path,
        publication: &PortablePublication,
    ) -> Result<String, SnapshotError>;
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PublisherFailpoint {
    None,
    AfterPushBeforeLocalRef,
    AfterLocalRefBeforePortableInstall,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PendingGitPublication {
    schema: String,
    expected_parent: String,
    repository_revision: String,
    database: DatabaseId,
    generation: u64,
    authority_revision: String,
    clone: PathBuf,
}

/// Git-backed authority publisher. It never commits from the user's working tree: the exact
/// portable subtree is copied into an isolated clone of the trusted remote parent, committed, and
/// pushed without force. A concurrent remote update therefore fails before local authority is
/// installed.
pub struct ProcessGitAuthorityPublisher {
    executable: PathBuf,
    repository: PathBuf,
    trusted_remote_url: String,
    branch: String,
    portable_relative: PathBuf,
    staging_root: PathBuf,
    failpoint: PublisherFailpoint,
}

impl ProcessGitAuthorityPublisher {
    pub fn new(
        executable: impl Into<PathBuf>,
        repository: impl Into<PathBuf>,
        trusted_remote_url: impl Into<String>,
        branch: impl Into<String>,
        portable_relative: impl Into<PathBuf>,
        staging_root: impl Into<PathBuf>,
    ) -> Result<Self, SnapshotError> {
        let value = Self {
            executable: executable.into(),
            repository: repository.into(),
            trusted_remote_url: trusted_remote_url.into(),
            branch: branch.into(),
            portable_relative: portable_relative.into(),
            staging_root: staging_root.into(),
            failpoint: PublisherFailpoint::None,
        };
        if !value.executable.is_absolute()
            || !value.repository.is_absolute()
            || value.trusted_remote_url.trim().is_empty()
            || !safe_git_branch(&value.branch)
            || !safe_relative_path(&value.portable_relative)
            || !value.staging_root.is_absolute()
        {
            return Err(SnapshotError::InvalidObjectStore);
        }
        Ok(value)
    }

    pub fn set_failpoint(&mut self, failpoint: PublisherFailpoint) {
        self.failpoint = failpoint;
    }

    fn pending_path(&self) -> PathBuf {
        self.staging_root.join("pending-git-publication.json")
    }

    fn anchor_ref(publication: &PortablePublication) -> String {
        format!(
            "refs/tags/commonkit-authority/{}/{}-{}",
            publication.database,
            publication.generation,
            publication
                .authority_revision
                .strip_prefix("sha256:")
                .unwrap_or("invalid")
        )
    }

    fn remote_refs(&self, pattern: &str) -> Result<Vec<(String, String)>, SnapshotError> {
        let output = Command::new(&self.executable)
            .args(["ls-remote", self.trusted_remote_url.as_str(), pattern])
            .stdin(Stdio::null())
            .output()
            .map_err(|_| SnapshotError::ObjectStoreFailed)?;
        if !output.status.success() {
            return Err(SnapshotError::StalePortableAuthority);
        }
        std::str::from_utf8(&output.stdout)
            .map_err(|_| SnapshotError::TransactionIntegrity)?
            .lines()
            .map(|line| {
                let mut fields = line.split_whitespace();
                let revision = fields.next().filter(|value| valid_git_revision(value));
                let reference = fields.next();
                match (revision, reference, fields.next()) {
                    (Some(revision), Some(reference), None) => {
                        Ok((revision.into(), reference.into()))
                    }
                    _ => Err(SnapshotError::TransactionIntegrity),
                }
            })
            .collect()
    }

    pub fn verify_remote_trust_anchor(
        &self,
        database: &DatabaseId,
        generation: u64,
        authority_revision: &str,
    ) -> Result<(), SnapshotError> {
        validate_digest(authority_revision)?;
        let pattern = format!("refs/tags/commonkit-authority/{database}/*");
        let refs = self.remote_refs(&pattern)?;
        if refs.is_empty() {
            return Err(SnapshotError::PortableAuthorityRollback);
        }
        let mut parsed = refs
            .into_iter()
            .map(|(commit, reference)| {
                let leaf = reference
                    .rsplit('/')
                    .next()
                    .ok_or(SnapshotError::TransactionIntegrity)?;
                let (generation, revision) = leaf
                    .split_once('-')
                    .ok_or(SnapshotError::TransactionIntegrity)?;
                let generation = generation
                    .parse::<u64>()
                    .map_err(|_| SnapshotError::TransactionIntegrity)?;
                Ok((generation, format!("sha256:{revision}"), commit))
            })
            .collect::<Result<Vec<_>, SnapshotError>>()?;
        parsed.sort_by_key(|item| item.0);
        let latest = parsed
            .last()
            .ok_or(SnapshotError::PortableAuthorityRollback)?;
        let remote_head = self.trusted_remote_revision()?;
        if parsed
            .iter()
            .rev()
            .take_while(|item| item.0 == latest.0)
            .count()
            != 1
            || latest.0 != generation
            || latest.1 != authority_revision
        {
            return Err(SnapshotError::PortableAuthorityRollback);
        }
        let remote_ref = format!("refs/heads/{}", self.branch);
        self.git_status(
            &self.repository,
            &[
                "fetch",
                "--no-tags",
                self.trusted_remote_url.as_str(),
                &remote_ref,
            ],
        )?;
        let ancestry = Command::new(&self.executable)
            .arg("-C")
            .arg(&self.repository)
            .args(["merge-base", "--is-ancestor", &latest.2, &remote_head])
            .stdin(Stdio::null())
            .status()
            .map_err(|_| SnapshotError::ObjectStoreFailed)?;
        if !ancestry.success() {
            return Err(SnapshotError::PortableAuthorityRollback);
        }
        Ok(())
    }

    /// Establishes the first immutable authority anchor for a repository. This is deliberately
    /// separate from verification: callers must opt into genesis once, while every normal open
    /// fails closed when the anchor namespace is absent.
    pub fn bootstrap_remote_trust_anchor(
        &mut self,
        staged_portable_root: &Path,
        publication: &PortablePublication,
    ) -> Result<String, SnapshotError> {
        let pattern = format!("refs/tags/commonkit-authority/{}/*", publication.database);
        if !self.remote_refs(&pattern)?.is_empty() {
            return Err(SnapshotError::StalePortableAuthority);
        }
        let expected_parent = self.trusted_remote_revision()?;
        if self.checked_out_revision()? != expected_parent {
            return Err(SnapshotError::StalePortableAuthority);
        }
        self.publish(&expected_parent, staged_portable_root, publication)
    }

    pub fn recover_pending_publication(
        &mut self,
    ) -> Result<Option<RecoveredPortablePublication>, SnapshotError> {
        let path = self.pending_path();
        if !path.exists() {
            return Ok(None);
        }
        let pending: PendingGitPublication = serde_json::from_slice(
            &std::fs::read(&path).map_err(|_| SnapshotError::ObjectStoreFailed)?,
        )
        .map_err(|_| SnapshotError::TransactionIntegrity)?;
        if pending.schema != "commonkit.git-publication.v1"
            || !valid_git_revision(&pending.expected_parent)
            || !valid_git_revision(&pending.repository_revision)
            || !pending.clone.is_absolute()
        {
            return Err(SnapshotError::TransactionIntegrity);
        }
        let expected_clone_parent = pending
            .clone
            .parent()
            .and_then(|parent| parent.canonicalize().ok());
        if expected_clone_parent.as_deref() != self.staging_root.canonicalize().ok().as_deref()
            || !pending
                .clone
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with("git-authority-"))
        {
            return Err(SnapshotError::TransactionIntegrity);
        }
        let publication = PortablePublication {
            database: pending.database.clone(),
            generation: pending.generation,
            authority_revision: pending.authority_revision.clone(),
        };
        self.verify_remote_trust_anchor(
            &pending.database,
            pending.generation,
            &pending.authority_revision,
        )?;
        if self.trusted_remote_revision()? != pending.repository_revision {
            return Err(SnapshotError::PortableAuthorityRollback);
        }
        let remote_ref = format!("refs/heads/{}", self.branch);
        self.git_status(
            &pending.clone,
            &[
                "fetch",
                "--no-tags",
                self.trusted_remote_url.as_str(),
                &remote_ref,
            ],
        )?;
        self.git_status(
            &pending.clone,
            &[
                "checkout",
                "--force",
                "--detach",
                &pending.repository_revision,
            ],
        )?;
        self.git_status(
            &self.repository,
            &[
                "fetch",
                "--no-tags",
                self.trusted_remote_url.as_str(),
                &remote_ref,
            ],
        )?;
        let local_ref = format!("refs/heads/{}", self.branch);
        let current = self.git_output(&self.repository, &["rev-parse", &local_ref])?;
        if current != pending.repository_revision {
            self.git_status(
                &self.repository,
                &[
                    "update-ref",
                    &local_ref,
                    &pending.repository_revision,
                    &pending.expected_parent,
                ],
            )?;
        }
        let source = pending.clone.join(&self.portable_relative);
        let destination = self.repository.join(&self.portable_relative);
        if destination.exists() {
            std::fs::remove_dir_all(&destination).map_err(|_| SnapshotError::ObjectStoreFailed)?;
        }
        copy_portable_tree(&source, &destination)?;
        std::fs::remove_file(&path).map_err(|_| SnapshotError::ObjectStoreFailed)?;
        let _ = std::fs::remove_dir_all(&pending.clone);
        let _ = publication;
        Ok(Some(RecoveredPortablePublication {
            database: pending.database,
            generation: pending.generation,
            authority_revision: pending.authority_revision,
            repository_revision: pending.repository_revision,
        }))
    }

    pub fn checked_out_revision(&self) -> Result<String, SnapshotError> {
        self.git_output(&self.repository, &["rev-parse", "HEAD"])
    }

    pub fn trusted_remote_revision(&self) -> Result<String, SnapshotError> {
        let reference = format!("refs/heads/{}", self.branch);
        let output = Command::new(&self.executable)
            .args([
                "ls-remote",
                "--exit-code",
                self.trusted_remote_url.as_str(),
                reference.as_str(),
            ])
            .stdin(Stdio::null())
            .output()
            .map_err(|_| SnapshotError::ObjectStoreFailed)?;
        if !output.status.success() {
            return Err(SnapshotError::StalePortableAuthority);
        }
        let text =
            std::str::from_utf8(&output.stdout).map_err(|_| SnapshotError::TransactionIntegrity)?;
        let mut fields = text.split_whitespace();
        let revision = fields
            .next()
            .filter(|revision| valid_git_revision(revision))
            .ok_or(SnapshotError::TransactionIntegrity)?;
        let found_reference = fields.next().ok_or(SnapshotError::TransactionIntegrity)?;
        if found_reference != reference || fields.next().is_some() {
            return Err(SnapshotError::TransactionIntegrity);
        }
        Ok(revision.into())
    }

    fn git_output(&self, repository: &Path, args: &[&str]) -> Result<String, SnapshotError> {
        let output = Command::new(&self.executable)
            .arg("-C")
            .arg(repository)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|_| SnapshotError::ObjectStoreFailed)?;
        if !output.status.success() {
            return Err(SnapshotError::StalePortableAuthority);
        }
        let value = std::str::from_utf8(&output.stdout)
            .map_err(|_| SnapshotError::TransactionIntegrity)?
            .trim();
        if value.is_empty() {
            return Err(SnapshotError::TransactionIntegrity);
        }
        Ok(value.into())
    }

    fn git_status(&self, repository: &Path, args: &[&str]) -> Result<(), SnapshotError> {
        let output = Command::new(&self.executable)
            .arg("-C")
            .arg(repository)
            .args(args)
            .stdin(Stdio::null())
            .output()
            .map_err(|_| SnapshotError::ObjectStoreFailed)?;
        if output.status.success() {
            Ok(())
        } else {
            Err(SnapshotError::StalePortableAuthority)
        }
    }
}

impl PortableAuthorityPublisher for ProcessGitAuthorityPublisher {
    fn publish(
        &mut self,
        expected_parent: &str,
        staged_portable_root: &Path,
        publication: &PortablePublication,
    ) -> Result<String, SnapshotError> {
        if !valid_git_revision(expected_parent)
            || validate_digest(&publication.authority_revision).is_err()
            || self.checked_out_revision()? != expected_parent
            || self.trusted_remote_revision()? != expected_parent
        {
            return Err(SnapshotError::StalePortableAuthority);
        }
        std::fs::create_dir_all(&self.staging_root)
            .map_err(|_| SnapshotError::ObjectStoreFailed)?;
        let clone = self
            .staging_root
            .join(format!("git-authority-{:032x}", rand::random::<u128>()));
        let result = (|| {
            let status = Command::new(&self.executable)
                .args(["clone", "--no-checkout", "--single-branch", "--branch"])
                .arg(&self.branch)
                .arg(&self.trusted_remote_url)
                .arg(&clone)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .status()
                .map_err(|_| SnapshotError::ObjectStoreFailed)?;
            if !status.success()
                || self.git_output(&clone, &["rev-parse", "HEAD"])? != expected_parent
            {
                return Err(SnapshotError::StalePortableAuthority);
            }
            self.git_status(&clone, &["checkout", "--detach", expected_parent])?;
            let destination = clone.join(&self.portable_relative);
            if destination.exists() {
                std::fs::remove_dir_all(&destination)
                    .map_err(|_| SnapshotError::ObjectStoreFailed)?;
            }
            copy_portable_tree(staged_portable_root, &destination)?;
            let portable = self
                .portable_relative
                .to_str()
                .ok_or(SnapshotError::InvalidObjectStore)?;
            self.git_status(&clone, &["add", "--", portable])?;
            let staged_changes = Command::new(&self.executable)
                .arg("-C")
                .arg(&clone)
                .args(["diff", "--cached", "--quiet", "--exit-code"])
                .stdin(Stdio::null())
                .status()
                .map_err(|_| SnapshotError::ObjectStoreFailed)?;
            if !staged_changes.success() {
                if let Err(error) = self.git_status(
                    &clone,
                    &[
                        "-c",
                        "user.name=CommonKit",
                        "-c",
                        "user.email=commonkit@localhost",
                        "commit",
                        "-m",
                        "commonkit: publish portable authority",
                    ],
                ) {
                    let _ = std::fs::remove_file(self.pending_path());
                    return Err(error);
                }
            }
            let revision = self.git_output(&clone, &["rev-parse", "HEAD"])?;
            std::fs::create_dir_all(&self.staging_root)
                .map_err(|_| SnapshotError::ObjectStoreFailed)?;
            let pending = PendingGitPublication {
                schema: "commonkit.git-publication.v1".into(),
                expected_parent: expected_parent.into(),
                repository_revision: revision.clone(),
                database: publication.database.clone(),
                generation: publication.generation,
                authority_revision: publication.authority_revision.clone(),
                clone: clone.clone(),
            };
            atomic_write(
                &self.pending_path(),
                &serde_json::to_vec(&pending).map_err(|_| SnapshotError::TransactionIntegrity)?,
            )?;
            let destination_ref = format!("HEAD:refs/heads/{}", self.branch);
            let anchor_ref = format!("HEAD:{}", Self::anchor_ref(publication));
            self.git_status(
                &clone,
                &[
                    "push",
                    "--atomic",
                    self.trusted_remote_url.as_str(),
                    &destination_ref,
                    &anchor_ref,
                ],
            )?;
            if self.trusted_remote_revision()? != revision {
                return Err(SnapshotError::StalePortableAuthority);
            }
            if self.failpoint == PublisherFailpoint::AfterPushBeforeLocalRef {
                return Err(SnapshotError::Interrupted);
            }
            let remote_ref = format!("refs/heads/{}", self.branch);
            self.git_status(
                &self.repository,
                &[
                    "fetch",
                    "--no-tags",
                    self.trusted_remote_url.as_str(),
                    &remote_ref,
                ],
            )?;
            let local_ref = format!("refs/heads/{}", self.branch);
            self.git_status(
                &self.repository,
                &["update-ref", &local_ref, &revision, expected_parent],
            )?;
            if self.failpoint == PublisherFailpoint::AfterLocalRefBeforePortableInstall {
                return Err(SnapshotError::Interrupted);
            }
            let destination = self.repository.join(&self.portable_relative);
            if destination.exists() {
                std::fs::remove_dir_all(&destination)
                    .map_err(|_| SnapshotError::ObjectStoreFailed)?;
            }
            copy_portable_tree(&clone.join(&self.portable_relative), &destination)?;
            std::fs::remove_file(self.pending_path())
                .map_err(|_| SnapshotError::ObjectStoreFailed)?;
            Ok(revision)
        })();
        if !self.pending_path().exists() {
            let _ = std::fs::remove_dir_all(&clone);
        }
        result
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PortableAuthorityFailpoint {
    None,
    AfterHistoryWrite,
}

pub struct PortableAuthorityStore<'a, C: AuthenticatedCipher> {
    root: PathBuf,
    trusted_anchor_root: Option<PathBuf>,
    cipher: &'a C,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct TrustedAuthorityAnchor {
    schema: String,
    database: DatabaseId,
    authority_revision: String,
    generation: u64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct PortableAuthorityPublishIntent {
    schema: String,
    database: DatabaseId,
    previous_revision: Option<String>,
    next_revision: String,
    next_record: PortableAuthorityRecord,
}

struct PortableAuthorityLock(PathBuf);

impl Drop for PortableAuthorityLock {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
    }
}

impl<'a, C: AuthenticatedCipher> PortableAuthorityStore<'a, C> {
    pub fn open(root: impl Into<PathBuf>, cipher: &'a C) -> Result<Self, SnapshotError> {
        Self::open_internal(root.into(), None, cipher)
    }

    pub fn open_with_trusted_anchor(
        root: impl Into<PathBuf>,
        trusted_anchor_root: impl Into<PathBuf>,
        cipher: &'a C,
    ) -> Result<Self, SnapshotError> {
        Self::open_internal(root.into(), Some(trusted_anchor_root.into()), cipher)
    }

    fn open_internal(
        root: PathBuf,
        trusted_anchor_root: Option<PathBuf>,
        cipher: &'a C,
    ) -> Result<Self, SnapshotError> {
        std::fs::create_dir_all(root.join("authority"))
            .map_err(|_| SnapshotError::ObjectStoreFailed)?;
        std::fs::create_dir_all(root.join("snapshots"))
            .map_err(|_| SnapshotError::ObjectStoreFailed)?;
        if let Some(anchor) = &trusted_anchor_root {
            std::fs::create_dir_all(anchor).map_err(|_| SnapshotError::ObjectStoreFailed)?;
        }
        Ok(Self {
            root,
            trusted_anchor_root,
            cipher,
        })
    }

    fn path(&self, database: &DatabaseId) -> PathBuf {
        self.root.join("authority").join(format!("{database}.json"))
    }

    fn history_root(&self, database: &DatabaseId) -> PathBuf {
        self.root
            .join("authority-history")
            .join(database.to_string())
    }

    fn publish_intent_path(&self, database: &DatabaseId) -> PathBuf {
        self.root
            .join("authority-publish")
            .join(format!("{database}.json"))
    }

    fn publish_aad(database: &DatabaseId) -> String {
        format!("commonkit.portable-authority-publish.v1\0{database}")
    }

    fn write_record(
        &self,
        database: &DatabaseId,
        record: &PortableAuthorityRecord,
    ) -> Result<String, SnapshotError> {
        self.write_record_with_failpoint(database, record, PortableAuthorityFailpoint::None)
    }

    fn write_record_with_failpoint(
        &self,
        database: &DatabaseId,
        record: &PortableAuthorityRecord,
        failpoint: PortableAuthorityFailpoint,
    ) -> Result<String, SnapshotError> {
        let revision =
            sha256(&serde_jcs::to_vec(record).map_err(|_| SnapshotError::TransactionIntegrity)?);
        let intent_path = self.publish_intent_path(database);
        write_authenticated_envelope(
            &intent_path,
            &PortableAuthorityPublishIntent {
                schema: "commonkit.portable-authority-publish.v1".into(),
                database: database.clone(),
                previous_revision: record.previous_revision.clone(),
                next_revision: revision.clone(),
                next_record: record.clone(),
            },
            self.cipher,
            Self::publish_aad(database).as_bytes(),
        )?;
        // Publish immutable authenticated history before the mutable pointer. A crash between
        // these writes fails closed; it can never make an older pointer appear current.
        write_authenticated_envelope(
            &self
                .history_root(database)
                .join(format!("{}.json", &revision[7..])),
            record,
            self.cipher,
            Self::aad(database).as_bytes(),
        )?;
        if failpoint == PortableAuthorityFailpoint::AfterHistoryWrite {
            return Err(SnapshotError::Interrupted);
        }
        write_authenticated_envelope(
            &self.path(database),
            record,
            self.cipher,
            Self::aad(database).as_bytes(),
        )?;
        self.advance_trusted_anchor(database, record, &revision)?;
        std::fs::remove_file(intent_path).map_err(|_| SnapshotError::ObjectStoreFailed)?;
        Ok(revision)
    }

    fn repair_interrupted_publish(&self, database: &DatabaseId) -> Result<(), SnapshotError> {
        let intent_path = self.publish_intent_path(database);
        if !intent_path.exists() {
            return Ok(());
        }
        let (intent, _): (PortableAuthorityPublishIntent, String) = read_authenticated_envelope(
            &intent_path,
            self.cipher,
            Self::publish_aad(database).as_bytes(),
        )?;
        if intent.schema != "commonkit.portable-authority-publish.v1"
            || intent.database != *database
            || sha256(
                &serde_jcs::to_vec(&intent.next_record)
                    .map_err(|_| SnapshotError::TransactionIntegrity)?,
            ) != intent.next_revision
            || intent.next_record.previous_revision != intent.previous_revision
        {
            return Err(SnapshotError::TransactionIntegrity);
        }
        let history_path = self
            .history_root(database)
            .join(format!("{}.json", &intent.next_revision[7..]));
        let (history_record, history_revision): (PortableAuthorityRecord, String) =
            read_authenticated_envelope(
                &history_path,
                self.cipher,
                Self::aad(database).as_bytes(),
            )?;
        if history_revision != intent.next_revision || history_record != intent.next_record {
            return Err(SnapshotError::TransactionIntegrity);
        }
        if let Some(previous) = &intent.previous_revision {
            let (current_record, current_revision): (PortableAuthorityRecord, String) =
                read_authenticated_envelope(
                    &self.path(database),
                    self.cipher,
                    Self::aad(database).as_bytes(),
                )?;
            if current_revision == intent.next_revision && current_record == intent.next_record {
                self.advance_trusted_anchor(database, &intent.next_record, &intent.next_revision)?;
                return std::fs::remove_file(intent_path)
                    .map_err(|_| SnapshotError::ObjectStoreFailed);
            }
            if &current_revision != previous {
                return Err(SnapshotError::PortableAuthorityRollback);
            }
        } else if self.path(database).exists() {
            let (current_record, current_revision): (PortableAuthorityRecord, String) =
                read_authenticated_envelope(
                    &self.path(database),
                    self.cipher,
                    Self::aad(database).as_bytes(),
                )?;
            if current_revision == intent.next_revision && current_record == intent.next_record {
                self.advance_trusted_anchor(database, &intent.next_record, &intent.next_revision)?;
                return std::fs::remove_file(intent_path)
                    .map_err(|_| SnapshotError::ObjectStoreFailed);
            }
            return Err(SnapshotError::PortableAuthorityRollback);
        }
        write_authenticated_envelope(
            &self.path(database),
            &intent.next_record,
            self.cipher,
            Self::aad(database).as_bytes(),
        )?;
        self.advance_trusted_anchor(database, &intent.next_record, &intent.next_revision)?;
        std::fs::remove_file(intent_path).map_err(|_| SnapshotError::ObjectStoreFailed)
    }

    fn anchor_path(&self, database: &DatabaseId) -> Option<PathBuf> {
        self.trusted_anchor_root
            .as_ref()
            .map(|root| root.join(format!("{database}.json")))
    }

    fn anchor_aad(database: &DatabaseId) -> String {
        format!("commonkit.portable-authority-anchor.v1\0{database}")
    }

    fn advance_trusted_anchor(
        &self,
        database: &DatabaseId,
        record: &PortableAuthorityRecord,
        revision: &str,
    ) -> Result<(), SnapshotError> {
        let Some(path) = self.anchor_path(database) else {
            return Ok(());
        };
        if path.exists() {
            let (existing, _): (TrustedAuthorityAnchor, String) = read_authenticated_envelope(
                &path,
                self.cipher,
                Self::anchor_aad(database).as_bytes(),
            )?;
            if existing.database != *database
                || existing.schema != "commonkit.portable-authority-anchor.v1"
                || existing.generation > record.generation
                || (existing.generation == record.generation
                    && existing.authority_revision != revision)
            {
                return Err(SnapshotError::PortableAuthorityRollback);
            }
        }
        write_authenticated_envelope(
            &path,
            &TrustedAuthorityAnchor {
                schema: "commonkit.portable-authority-anchor.v1".into(),
                database: database.clone(),
                authority_revision: revision.into(),
                generation: record.generation,
            },
            self.cipher,
            Self::anchor_aad(database).as_bytes(),
        )?;
        Ok(())
    }

    fn validate_trusted_anchor(
        &self,
        database: &DatabaseId,
        record: &PortableAuthorityRecord,
        revision: &str,
    ) -> Result<(), SnapshotError> {
        let Some(path) = self.anchor_path(database) else {
            return Ok(());
        };
        if !path.exists() {
            return self.advance_trusted_anchor(database, record, revision);
        }
        let (anchor, _): (TrustedAuthorityAnchor, String) =
            read_authenticated_envelope(&path, self.cipher, Self::anchor_aad(database).as_bytes())?;
        if anchor.schema != "commonkit.portable-authority-anchor.v1"
            || anchor.database != *database
            || anchor.generation > record.generation
            || (anchor.generation == record.generation && anchor.authority_revision != revision)
        {
            return Err(SnapshotError::PortableAuthorityRollback);
        }
        self.advance_trusted_anchor(database, record, revision)
    }

    fn lock(&self, database: &DatabaseId) -> Result<PortableAuthorityLock, SnapshotError> {
        let path = self
            .root
            .join("authority")
            .join(format!(".{database}.lock"));
        for _ in 0..500 {
            match std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(&path)
            {
                Ok(_) => return Ok(PortableAuthorityLock(path)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                    std::thread::sleep(Duration::from_millis(2));
                }
                Err(_) => return Err(SnapshotError::ObjectStoreFailed),
            }
        }
        Err(SnapshotError::StalePortableAuthority)
    }

    fn aad(database: &DatabaseId) -> String {
        format!("commonkit.portable-snapshot-authority.v1\0{database}")
    }

    pub fn initialize(
        &self,
        database: &DatabaseId,
        writer: &str,
    ) -> Result<VersionedPortableAuthority, SnapshotError> {
        if writer.is_empty() {
            return Err(SnapshotError::TransactionIntegrity);
        }
        let _lock = self.lock(database)?;
        if self.path(database).exists() {
            // The portable record is authoritative. A newly cloned machine commonly has a
            // different local target ID and must not reinterpret that as initialization input.
            return self.read_unlocked(database);
        }
        // Missing mutable authority beside immutable descriptors or authority history is
        // deletion/rollback, not a fresh kit.
        if self.history_root(database).exists()
            || std::fs::read_dir(self.root.join("snapshots"))
                .map_err(|_| SnapshotError::ObjectStoreFailed)?
                .next()
                .is_some()
        {
            return Err(SnapshotError::PortableAuthorityRollback);
        }
        let record = PortableAuthorityRecord {
            schema: "commonkit.portable-snapshot-authority.v1".into(),
            database: database.clone(),
            current_writer: writer.into(),
            accepted_head: None,
            accepted_descriptor: None,
            generation: 0,
            previous_revision: None,
        };
        let revision = self.write_record(database, &record)?;
        Ok(VersionedPortableAuthority { record, revision })
    }

    pub fn read(&self, database: &DatabaseId) -> Result<VersionedPortableAuthority, SnapshotError> {
        self.read_unlocked(database)
    }

    /// Reads authority only when the checked-out Git commit is still the independently fetched
    /// trusted remote branch head. This is the fresh-machine/lagging-clone replay boundary; callers
    /// must obtain `trusted_remote_revision` from the configured remote, never from local Git state.
    pub fn read_at_repository_revision(
        &self,
        database: &DatabaseId,
        checked_out_repository_revision: &str,
        trusted_remote_revision: &str,
    ) -> Result<VersionedPortableAuthority, SnapshotError> {
        if checked_out_repository_revision.trim().is_empty()
            || checked_out_repository_revision != trusted_remote_revision
        {
            return Err(SnapshotError::PortableAuthorityRollback);
        }
        self.read_unlocked(database)
    }

    fn read_unlocked(
        &self,
        database: &DatabaseId,
    ) -> Result<VersionedPortableAuthority, SnapshotError> {
        self.repair_interrupted_publish(database)?;
        let (record, revision): (PortableAuthorityRecord, String) = read_authenticated_envelope(
            &self.path(database),
            self.cipher,
            Self::aad(database).as_bytes(),
        )?;
        if record.schema != "commonkit.portable-snapshot-authority.v1"
            || record.database != *database
            || record.current_writer.is_empty()
            || record.accepted_head.is_some() != record.accepted_descriptor.is_some()
            || (record.generation == 0 && record.previous_revision.is_some())
            || (record.generation > 0 && record.previous_revision.is_none())
        {
            return Err(SnapshotError::TransactionIntegrity);
        }
        if let Some(head) = &record.accepted_head {
            validate_digest(head)?;
        }
        if let Some(previous_revision) = &record.previous_revision {
            validate_digest(previous_revision)?;
        }
        if let Some(descriptor) = &record.accepted_descriptor {
            validate_digest(descriptor)?;
            let bytes = std::fs::read(
                self.root
                    .join("snapshots")
                    .join(format!("{}.json", &descriptor[7..])),
            )
            .map_err(|_| SnapshotError::PortableAuthorityRollback)?;
            if sha256(&bytes) != *descriptor {
                return Err(SnapshotError::PortableAuthorityRollback);
            }
        }
        self.validate_history_tip(database, &record, &revision)?;
        self.validate_trusted_anchor(database, &record, &revision)?;
        Ok(VersionedPortableAuthority { record, revision })
    }

    fn validate_history_tip(
        &self,
        database: &DatabaseId,
        current: &PortableAuthorityRecord,
        current_revision: &str,
    ) -> Result<(), SnapshotError> {
        let history_root = self.history_root(database);
        let mut history = BTreeMap::<String, PortableAuthorityRecord>::new();
        for entry in std::fs::read_dir(&history_root)
            .map_err(|_| SnapshotError::PortableAuthorityRollback)?
        {
            let entry = entry.map_err(|_| SnapshotError::PortableAuthorityRollback)?;
            if !entry
                .file_type()
                .map_err(|_| SnapshotError::PortableAuthorityRollback)?
                .is_file()
            {
                return Err(SnapshotError::PortableAuthorityRollback);
            }
            let (record, revision): (PortableAuthorityRecord, String) =
                read_authenticated_envelope(
                    &entry.path(),
                    self.cipher,
                    Self::aad(database).as_bytes(),
                )
                .map_err(|_| SnapshotError::PortableAuthorityRollback)?;
            if entry.file_name().to_string_lossy() != format!("{}.json", &revision[7..])
                || record.database != *database
                || history.insert(revision, record).is_some()
            {
                return Err(SnapshotError::PortableAuthorityRollback);
            }
        }
        let (tip_revision, tip) = history
            .iter()
            .max_by_key(|(_, record)| record.generation)
            .ok_or(SnapshotError::PortableAuthorityRollback)?;
        if history
            .values()
            .filter(|record| record.generation == tip.generation)
            .count()
            != 1
            || tip_revision != current_revision
            || tip != current
        {
            return Err(SnapshotError::PortableAuthorityRollback);
        }
        let mut cursor_revision = tip_revision;
        let mut cursor = tip;
        while cursor.generation > 0 {
            let previous = cursor
                .previous_revision
                .as_ref()
                .and_then(|revision| history.get_key_value(revision))
                .ok_or(SnapshotError::PortableAuthorityRollback)?;
            if previous.1.generation + 1 != cursor.generation {
                return Err(SnapshotError::PortableAuthorityRollback);
            }
            cursor_revision = previous.0;
            cursor = previous.1;
        }
        if cursor.generation != 0
            || cursor.previous_revision.is_some()
            || cursor_revision.is_empty()
        {
            return Err(SnapshotError::PortableAuthorityRollback);
        }
        Ok(())
    }

    pub fn compare_and_swap_writer(
        &self,
        database: &DatabaseId,
        expected_revision: &str,
        expected_writer: &str,
        candidate_writer: &str,
    ) -> Result<VersionedPortableAuthority, SnapshotError> {
        self.compare_and_swap_with_failpoint(
            database,
            expected_revision,
            PortableAuthorityFailpoint::None,
            |record| {
                if record.current_writer != expected_writer || candidate_writer.is_empty() {
                    return Err(SnapshotError::StalePortableAuthority);
                }
                record.current_writer = candidate_writer.into();
                Ok(())
            },
        )
    }

    pub fn compare_and_swap_writer_with_failpoint(
        &self,
        database: &DatabaseId,
        expected_revision: &str,
        expected_writer: &str,
        candidate_writer: &str,
        failpoint: PortableAuthorityFailpoint,
    ) -> Result<VersionedPortableAuthority, SnapshotError> {
        self.compare_and_swap_with_failpoint(database, expected_revision, failpoint, |record| {
            if record.current_writer != expected_writer || candidate_writer.is_empty() {
                return Err(SnapshotError::StalePortableAuthority);
            }
            record.current_writer = candidate_writer.into();
            Ok(())
        })
    }

    pub fn compare_and_swap_writer_published(
        &self,
        database: &DatabaseId,
        expected_revision: &str,
        expected_writer: &str,
        candidate_writer: &str,
        expected_repository_parent: &str,
        publisher: &mut impl PortableAuthorityPublisher,
    ) -> Result<PublishedPortableAuthority, SnapshotError> {
        self.compare_and_swap_published(
            database,
            expected_revision,
            expected_repository_parent,
            publisher,
            |record| {
                if record.current_writer != expected_writer || candidate_writer.is_empty() {
                    return Err(SnapshotError::StalePortableAuthority);
                }
                record.current_writer = candidate_writer.into();
                Ok(())
            },
        )
    }

    pub fn compare_and_swap_head_published(
        &self,
        database: &DatabaseId,
        expected_revision: &str,
        writer: &str,
        accepted_head: &str,
        accepted_descriptor: &str,
        expected_repository_parent: &str,
        publisher: &mut impl PortableAuthorityPublisher,
    ) -> Result<PublishedPortableAuthority, SnapshotError> {
        validate_digest(accepted_head)?;
        validate_digest(accepted_descriptor)?;
        self.compare_and_swap_published(
            database,
            expected_revision,
            expected_repository_parent,
            publisher,
            |record| {
                if record.current_writer != writer {
                    return Err(SnapshotError::StalePortableAuthority);
                }
                record.accepted_head = Some(accepted_head.into());
                record.accepted_descriptor = Some(accepted_descriptor.into());
                Ok(())
            },
        )
    }

    fn compare_and_swap_published(
        &self,
        database: &DatabaseId,
        expected_revision: &str,
        expected_repository_parent: &str,
        publisher: &mut impl PortableAuthorityPublisher,
        update: impl FnOnce(&mut PortableAuthorityRecord) -> Result<(), SnapshotError>,
    ) -> Result<PublishedPortableAuthority, SnapshotError> {
        if expected_repository_parent.trim().is_empty() {
            return Err(SnapshotError::StalePortableAuthority);
        }
        let stage = self
            .root
            .parent()
            .ok_or(SnapshotError::InvalidObjectStore)?
            .join(format!(".authority-stage-{:032x}", rand::random::<u128>()));
        if let Err(error) = copy_portable_tree(&self.root, &stage) {
            let _ = std::fs::remove_dir_all(&stage);
            return Err(error);
        }
        let staged_store = PortableAuthorityStore::open(&stage, self.cipher)?;
        let staged = match staged_store.compare_and_swap_with_failpoint(
            database,
            expected_revision,
            PortableAuthorityFailpoint::None,
            update,
        ) {
            Ok(value) => value,
            Err(error) => {
                let _ = std::fs::remove_dir_all(&stage);
                return Err(error);
            }
        };
        let repository_revision = match publisher.publish(
            expected_repository_parent,
            &stage,
            &PortablePublication {
                database: database.clone(),
                generation: staged.record.generation,
                authority_revision: staged.revision.clone(),
            },
        ) {
            Ok(revision) if !revision.trim().is_empty() => revision,
            Ok(_) => {
                let _ = std::fs::remove_dir_all(&stage);
                return Err(SnapshotError::StalePortableAuthority);
            }
            Err(error) => {
                let _ = std::fs::remove_dir_all(&stage);
                return Err(error);
            }
        };

        let history_source = staged_store
            .history_root(database)
            .join(format!("{}.json", &staged.revision[7..]));
        let history_destination = self
            .history_root(database)
            .join(format!("{}.json", &staged.revision[7..]));
        copy_verified_file(&history_source, &history_destination)?;
        copy_verified_file(&staged_store.path(database), &self.path(database))?;
        self.advance_trusted_anchor(database, &staged.record, &staged.revision)?;
        std::fs::remove_dir_all(&stage).map_err(|_| SnapshotError::ObjectStoreFailed)?;
        Ok(PublishedPortableAuthority {
            authority: staged,
            repository_revision,
        })
    }

    pub fn compare_and_swap_head(
        &self,
        database: &DatabaseId,
        expected_revision: &str,
        writer: &str,
        accepted_head: &str,
        accepted_descriptor: &str,
    ) -> Result<VersionedPortableAuthority, SnapshotError> {
        validate_digest(accepted_head)?;
        validate_digest(accepted_descriptor)?;
        self.compare_and_swap_with_failpoint(
            database,
            expected_revision,
            PortableAuthorityFailpoint::None,
            |record| {
                if record.current_writer != writer {
                    return Err(SnapshotError::StalePortableAuthority);
                }
                record.accepted_head = Some(accepted_head.into());
                record.accepted_descriptor = Some(accepted_descriptor.into());
                Ok(())
            },
        )
    }

    fn compare_and_swap_with_failpoint(
        &self,
        database: &DatabaseId,
        expected_revision: &str,
        failpoint: PortableAuthorityFailpoint,
        update: impl FnOnce(&mut PortableAuthorityRecord) -> Result<(), SnapshotError>,
    ) -> Result<VersionedPortableAuthority, SnapshotError> {
        let _lock = self.lock(database)?;
        let current = self.read_unlocked(database)?;
        if current.revision != expected_revision {
            return Err(SnapshotError::StalePortableAuthority);
        }
        let mut record = current.record;
        update(&mut record)?;
        record.generation = record
            .generation
            .checked_add(1)
            .ok_or(SnapshotError::TransactionIntegrity)?;
        record.previous_revision = Some(current.revision);
        let revision = self.write_record_with_failpoint(database, &record, failpoint)?;
        Ok(VersionedPortableAuthority { record, revision })
    }
}

impl AuthorityStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, SnapshotError> {
        let root = root.into();
        std::fs::create_dir_all(root.join("promotions"))
            .map_err(|_| SnapshotError::ObjectStoreFailed)?;
        Ok(Self { root })
    }

    pub fn initialize(&self, database: &DatabaseId, writer: &str) -> Result<(), SnapshotError> {
        let path = self.root.join(format!("authority-{database}.json"));
        if path.exists() {
            let existing: (Authority, String) = read_envelope(&path)?;
            return match existing.0 {
                Authority::Writer { target } if target == writer => Ok(()),
                Authority::Writer { target } => Err(SnapshotError::WriterAlreadyAssigned(target)),
                Authority::Unassigned => Err(SnapshotError::TransactionIntegrity),
            };
        }
        write_envelope(
            &path,
            &Authority::Writer {
                target: writer.into(),
            },
        )?;
        Ok(())
    }

    pub fn authority(&self, database: &DatabaseId) -> Result<Authority, SnapshotError> {
        read_envelope(&self.root.join(format!("authority-{database}.json"))).map(|pair| pair.0)
    }

    /// Updates the target-local cache from authenticated portable authority. This cache never
    /// decides writer ownership; it exists only to preserve legacy promotion receipt recovery.
    pub fn synchronize_from_portable(
        &self,
        database: &DatabaseId,
        writer: &str,
    ) -> Result<(), SnapshotError> {
        write_envelope(
            &self.root.join(format!("authority-{database}.json")),
            &Authority::Writer {
                target: writer.into(),
            },
        )
        .map(|_| ())
    }

    pub fn promote(&self, plan: PromotionPlan) -> Result<PromotionReceipt, SnapshotError> {
        self.promote_with_failpoint(plan, PromotionFailpoint::None)
    }

    /// Durably records a validated promotion before any shared portable authority is changed.
    /// Once this returns, recovery needs only this plan and the authenticated portable writer.
    pub fn prepare_promotion(&self, plan: PromotionPlan) -> Result<String, SnapshotError> {
        self.validate_promotion(&plan)?;
        let run = self.root.join("promotions").join(&plan.run_id);
        std::fs::create_dir_all(&run).map_err(|_| SnapshotError::ObjectStoreFailed)?;
        let path = run.join("plan.json");
        if path.exists() {
            let (existing, digest): (PromotionPlan, String) = read_envelope(&path)?;
            return if existing == plan {
                Ok(digest)
            } else {
                Err(SnapshotError::TransactionIntegrity)
            };
        }
        let digest = write_envelope(&path, &plan)?;
        write_envelope(&run.join("phase.json"), &PromotionPhase::Prepared)?;
        Ok(digest)
    }

    pub fn confirm_portable_promotion(
        &self,
        run_id: &str,
        portable_writer: &str,
        portable_revision: &str,
    ) -> Result<(), SnapshotError> {
        validate_digest(portable_revision)?;
        let run = self.root.join("promotions").join(run_id);
        let (plan, _): (PromotionPlan, String) = read_envelope(&run.join("plan.json"))?;
        if plan.run_id != run_id || plan.candidate_writer != portable_writer {
            return Err(SnapshotError::StalePortableAuthority);
        }
        match self.promotion_phase(&run)? {
            PromotionPhase::Prepared => {
                write_envelope(
                    &run.join("phase.json"),
                    &PromotionPhase::CasConfirmed {
                        portable_revision: portable_revision.into(),
                    },
                )?;
                Ok(())
            }
            PromotionPhase::CasConfirmed {
                portable_revision: existing,
            } if existing == portable_revision => Ok(()),
            _ => Err(SnapshotError::TransactionIntegrity),
        }
    }

    /// Commits an already prepared promotion after the portable writer CAS has succeeded.
    pub fn commit_prepared_promotion(
        &self,
        run_id: &str,
    ) -> Result<PromotionReceipt, SnapshotError> {
        let run = self.root.join("promotions").join(run_id);
        let (plan, plan_digest): (PromotionPlan, String) = read_envelope(&run.join("plan.json"))?;
        if plan.run_id != run_id {
            return Err(SnapshotError::TransactionIntegrity);
        }
        if !matches!(
            self.promotion_phase(&run)?,
            PromotionPhase::CasConfirmed { .. }
        ) {
            return Err(SnapshotError::TransactionIntegrity);
        }
        if run.join("receipt.json").exists() {
            let (receipt, _): (PromotionReceipt, String) =
                read_envelope(&run.join("receipt.json"))?;
            if receipt.plan_digest != plan_digest {
                return Err(SnapshotError::TransactionIntegrity);
            }
            return Ok(receipt);
        }
        write_envelope(
            &self.root.join(format!("authority-{}.json", plan.database)),
            &Authority::Writer {
                target: plan.candidate_writer.clone(),
            },
        )?;
        self.finish_promotion(plan, plan_digest, &run)
    }

    /// Validates a promotion without writing plans, receipts, or authority. Callers coordinating
    /// a portable compare-and-swap use this before publishing the portable writer transition.
    pub fn validate_promotion(&self, plan: &PromotionPlan) -> Result<(), SnapshotError> {
        if plan.schema != "commonkit.promotion-plan.v1"
            || plan.run_id.is_empty()
            || !plan
                .run_id
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
            || plan.candidate_writer.is_empty()
            || plan.current_writer_digest != plan.latest_snapshot_digest
        {
            return Err(SnapshotError::UnsnapshottedWriterChanges);
        }
        if plan.candidate_digest != plan.latest_snapshot_digest {
            return Err(SnapshotError::CandidateDoesNotMatchSnapshot);
        }
        let authority = self.authority(&plan.database)?;
        if authority
            != (Authority::Writer {
                target: plan.previous_writer.clone(),
            })
        {
            return Err(SnapshotError::TransactionIntegrity);
        }
        Ok(())
    }

    pub fn promote_with_failpoint(
        &self,
        plan: PromotionPlan,
        failpoint: PromotionFailpoint,
    ) -> Result<PromotionReceipt, SnapshotError> {
        self.prepare_promotion(plan.clone())?;
        self.confirm_portable_promotion(
            &plan.run_id,
            &plan.candidate_writer,
            "sha256:0000000000000000000000000000000000000000000000000000000000000000",
        )?;
        write_envelope(
            &self.root.join(format!("authority-{}.json", plan.database)),
            &Authority::Writer {
                target: plan.candidate_writer.clone(),
            },
        )?;
        if failpoint == PromotionFailpoint::AfterAuthorityWrite {
            return Err(SnapshotError::Interrupted);
        }
        self.commit_prepared_promotion(&plan.run_id)
    }

    pub fn recover_promotion(&self, run_id: &str) -> Result<PromotionReceipt, SnapshotError> {
        let run = self.root.join("promotions").join(run_id);
        let (plan, _): (PromotionPlan, String) = read_envelope(&run.join("plan.json"))?;
        let writer = match self.authority(&plan.database)? {
            Authority::Writer { target } => target,
            Authority::Unassigned => return Err(SnapshotError::TransactionIntegrity),
        };
        if matches!(self.promotion_phase(&run)?, PromotionPhase::Prepared) {
            self.confirm_portable_promotion(
                run_id,
                &writer,
                "sha256:0000000000000000000000000000000000000000000000000000000000000000",
            )?;
        }
        self.commit_prepared_promotion(run_id)
    }

    pub fn recover_promotion_against_portable(
        &self,
        run_id: &str,
        portable_writer: &str,
        portable_revision: &str,
    ) -> Result<PromotionRecovery, SnapshotError> {
        validate_digest(portable_revision)?;
        let run = self.root.join("promotions").join(run_id);
        let (plan, _): (PromotionPlan, String) = read_envelope(&run.join("plan.json"))?;
        match self.promotion_phase(&run)? {
            PromotionPhase::Prepared if portable_writer == plan.previous_writer => {
                write_envelope(
                    &run.join("phase.json"),
                    &PromotionPhase::Aborted {
                        portable_revision: portable_revision.into(),
                    },
                )?;
                Ok(PromotionRecovery::Aborted)
            }
            PromotionPhase::Prepared if portable_writer == plan.candidate_writer => {
                self.confirm_portable_promotion(run_id, portable_writer, portable_revision)?;
                self.commit_prepared_promotion(run_id)
                    .map(PromotionRecovery::Committed)
            }
            PromotionPhase::CasConfirmed { .. } if portable_writer == plan.candidate_writer => self
                .commit_prepared_promotion(run_id)
                .map(PromotionRecovery::Committed),
            PromotionPhase::Aborted { .. } if portable_writer == plan.previous_writer => {
                Ok(PromotionRecovery::Aborted)
            }
            _ => Err(SnapshotError::StalePortableAuthority),
        }
    }

    pub fn prepared_promotion(&self, run_id: &str) -> Result<PromotionPlan, SnapshotError> {
        let run = self.root.join("promotions").join(run_id);
        let (plan, _): (PromotionPlan, String) = read_envelope(&run.join("plan.json"))?;
        if plan.run_id != run_id {
            return Err(SnapshotError::TransactionIntegrity);
        }
        Ok(plan)
    }

    /// Enumerates authenticated promotion plans that do not yet have a receipt. Any malformed,
    /// symlinked, or mismatched entry fails the entire scan closed.
    pub fn unfinished_promotions(&self) -> Result<Vec<String>, SnapshotError> {
        let mut runs = Vec::new();
        for entry in std::fs::read_dir(self.root.join("promotions"))
            .map_err(|_| SnapshotError::ObjectStoreFailed)?
        {
            let entry = entry.map_err(|_| SnapshotError::TransactionIntegrity)?;
            let metadata = std::fs::symlink_metadata(entry.path())
                .map_err(|_| SnapshotError::TransactionIntegrity)?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(SnapshotError::TransactionIntegrity);
            }
            let run_id = entry
                .file_name()
                .into_string()
                .map_err(|_| SnapshotError::TransactionIntegrity)?;
            let (plan, _): (PromotionPlan, String) =
                read_envelope(&entry.path().join("plan.json"))?;
            if plan.run_id != run_id {
                return Err(SnapshotError::TransactionIntegrity);
            }
            if matches!(
                self.promotion_phase(&entry.path())?,
                PromotionPhase::Aborted { .. }
            ) {
                continue;
            }
            if !entry.path().join("receipt.json").exists() {
                runs.push(run_id);
            } else {
                let (receipt, _): (PromotionReceipt, String) =
                    read_envelope(&entry.path().join("receipt.json"))?;
                if receipt.run_id != plan.run_id {
                    return Err(SnapshotError::TransactionIntegrity);
                }
            }
        }
        runs.sort();
        Ok(runs)
    }

    fn promotion_phase(&self, run: &Path) -> Result<PromotionPhase, SnapshotError> {
        let path = run.join("phase.json");
        if !path.exists() {
            return Ok(PromotionPhase::Prepared);
        }
        read_envelope(&path).map(|pair| pair.0)
    }

    fn finish_promotion(
        &self,
        plan: PromotionPlan,
        plan_digest: String,
        run: &Path,
    ) -> Result<PromotionReceipt, SnapshotError> {
        let receipt = PromotionReceipt {
            schema: "commonkit.promotion-receipt.v1".into(),
            run_id: plan.run_id,
            plan_digest,
            database: plan.database,
            previous_writer: plan.previous_writer,
            authoritative_writer: plan.candidate_writer,
            snapshot_digest: plan.latest_snapshot_digest,
        };
        write_envelope(&run.join("receipt.json"), &receipt)?;
        Ok(receipt)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RestoreState {
    Planned,
    Prepared,
    Swapping,
    Swapped,
    Verified,
    RolledBack,
    Recoverable,
}

#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct IntegrityEnvelope<T> {
    value: T,
    digest: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    authentication: Option<Vec<u8>>,
}

/// Target-local content-addressed store. Values are already authenticated ciphertext; reads
/// validate the filename digest before returning bytes.
#[derive(Clone, Debug)]
pub struct DurableObjectStore {
    root: PathBuf,
}

impl DurableObjectStore {
    pub fn open(root: impl Into<PathBuf>) -> Result<Self, SnapshotError> {
        let root = root.into();
        std::fs::create_dir_all(&root).map_err(|_| SnapshotError::ObjectStoreFailed)?;
        Ok(Self { root })
    }

    fn path(&self, digest: &str) -> Result<PathBuf, SnapshotError> {
        validate_digest(digest)?;
        Ok(self.root.join(&digest[7..]))
    }
}

impl ObjectStore for DurableObjectStore {
    fn put(&mut self, key: &str, ciphertext: &[u8]) -> Result<(), SnapshotError> {
        if sha256(ciphertext) != key {
            return Err(SnapshotError::IntegrityMismatch);
        }
        let path = self.path(key)?;
        if path.exists() {
            return self.get(key).map(|_| ());
        }
        atomic_write(&path, ciphertext)
    }

    fn get(&self, key: &str) -> Result<Vec<u8>, SnapshotError> {
        let bytes = std::fs::read(self.path(key)?)
            .map_err(|_| SnapshotError::ObjectNotFound(key.into()))?;
        if sha256(&bytes) != key {
            return Err(SnapshotError::IntegrityMismatch);
        }
        Ok(bytes)
    }
}

/// Durable, restart-reconstructable restore transaction. Provider/object fetch and decryption
/// happen before mutation. Recovery consumes only the persisted plan, receipt, staged database,
/// and encrypted preimage object.
pub struct DurableRestore<'a, C> {
    root: PathBuf,
    cipher: &'a C,
    objects: DurableObjectStore,
}

impl<'a, C: AuthenticatedCipher> DurableRestore<'a, C> {
    pub fn open(root: impl Into<PathBuf>, cipher: &'a C) -> Result<Self, SnapshotError> {
        let root = root.into();
        std::fs::create_dir_all(root.join("runs")).map_err(|_| SnapshotError::ObjectStoreFailed)?;
        let objects = DurableObjectStore::open(root.join("objects"))?;
        Ok(Self {
            root,
            cipher,
            objects,
        })
    }

    pub fn execute(
        &mut self,
        plan: RestorePlan,
        manifest: &SnapshotManifest,
        snapshot_objects: &impl ObjectStore,
        database_path: &Path,
        lifecycle: &mut impl DatabaseLifecycle,
        failpoint: RestoreFailpoint,
    ) -> Result<RestoreReceipt, SnapshotError> {
        validate_plan(&plan, manifest)?;
        let run = self.root.join("runs").join(&plan.run_id);
        std::fs::create_dir_all(&run).map_err(|_| SnapshotError::ObjectStoreFailed)?;
        let plan_digest = write_authenticated_envelope(
            &run.join("plan.json"),
            &plan,
            self.cipher,
            restore_auth_aad("plan", &plan.run_id).as_bytes(),
        )?;
        let mut receipt = RestoreReceipt {
            schema: "commonkit.restore-receipt.v1".into(),
            run_id: plan.run_id.clone(),
            plan_digest,
            state: RestoreState::Planned,
            preimage_object_digest: None,
            resulting_content_digest: None,
        };
        self.write_receipt(&run, &receipt)?;

        let plaintext =
            SnapshotService::new(self.cipher).verify_and_decrypt(manifest, snapshot_objects)?;
        let staged = run.join("staged.database");
        atomic_write(&staged, &plaintext)?;
        if sha256(&std::fs::read(&staged).map_err(|_| SnapshotError::DatabaseFailed)?)
            != plan.expected_content_digest
        {
            return Err(SnapshotError::IntegrityMismatch);
        }
        let previous = std::fs::read(database_path).map_err(|_| SnapshotError::DatabaseFailed)?;
        let aad = format!("commonkit.restore-preimage.v1\0{}", plan.run_id);
        let encrypted = self.cipher.seal(&previous, aad.as_bytes())?;
        let preimage_digest = sha256(&encrypted);
        self.objects.put(&preimage_digest, &encrypted)?;
        receipt.preimage_object_digest = Some(preimage_digest);
        receipt.state = RestoreState::Prepared;
        self.write_receipt(&run, &receipt)?;

        receipt.state = RestoreState::Swapping;
        self.write_receipt(&run, &receipt)?;
        lifecycle.stop()?;
        replace_file(database_path, &staged)?;
        receipt.state = RestoreState::Swapped;
        self.write_receipt(&run, &receipt)?;
        if failpoint == RestoreFailpoint::AfterSwap {
            return Err(SnapshotError::Interrupted);
        }
        if verify_database(database_path, &plan.expected_content_digest, manifest).is_err() {
            self.rollback(&plan, &mut receipt, database_path, lifecycle)?;
            return Err(SnapshotError::RestoreVerificationFailed);
        }
        lifecycle.start()?;
        receipt.state = RestoreState::Verified;
        receipt.resulting_content_digest = Some(plan.expected_content_digest);
        self.write_receipt(&run, &receipt)?;
        Ok(receipt)
    }

    pub fn recover(
        &mut self,
        run_id: &str,
        database_path: &Path,
        lifecycle: &mut impl DatabaseLifecycle,
    ) -> Result<RestoreReceipt, SnapshotError> {
        let run = self.root.join("runs").join(run_id);
        let (plan, plan_digest): (RestorePlan, String) = read_authenticated_envelope(
            &run.join("plan.json"),
            self.cipher,
            restore_auth_aad("plan", run_id).as_bytes(),
        )?;
        let (mut receipt, _): (RestoreReceipt, String) = read_authenticated_envelope(
            &run.join("receipt.json"),
            self.cipher,
            restore_auth_aad("receipt", run_id).as_bytes(),
        )?;
        if receipt.run_id != plan.run_id || receipt.plan_digest != plan_digest {
            return Err(SnapshotError::TransactionIntegrity);
        }
        match receipt.state {
            RestoreState::Verified | RestoreState::RolledBack => Ok(receipt),
            RestoreState::Planned | RestoreState::Prepared => {
                let _ = std::fs::remove_file(run.join("staged.database"));
                receipt.state = RestoreState::RolledBack;
                self.write_receipt(&run, &receipt)?;
                Ok(receipt)
            }
            RestoreState::Swapping | RestoreState::Swapped | RestoreState::Recoverable => {
                self.rollback(&plan, &mut receipt, database_path, lifecycle)?;
                Ok(receipt)
            }
        }
    }

    /// Returns authenticated plans for every non-terminal restore transaction. Recovery callers
    /// can use the database ID to select the configured live path and lifecycle after restart.
    pub fn unfinished_runs(&self) -> Result<Vec<RestorePlan>, SnapshotError> {
        let mut plans = Vec::new();
        for entry in std::fs::read_dir(self.root.join("runs"))
            .map_err(|_| SnapshotError::ObjectStoreFailed)?
        {
            let entry = entry.map_err(|_| SnapshotError::TransactionIntegrity)?;
            let metadata = std::fs::symlink_metadata(entry.path())
                .map_err(|_| SnapshotError::TransactionIntegrity)?;
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(SnapshotError::TransactionIntegrity);
            }
            let run_id = entry
                .file_name()
                .into_string()
                .map_err(|_| SnapshotError::TransactionIntegrity)?;
            let (plan, plan_digest): (RestorePlan, String) = read_authenticated_envelope(
                &entry.path().join("plan.json"),
                self.cipher,
                restore_auth_aad("plan", &run_id).as_bytes(),
            )?;
            let (receipt, _): (RestoreReceipt, String) = read_authenticated_envelope(
                &entry.path().join("receipt.json"),
                self.cipher,
                restore_auth_aad("receipt", &run_id).as_bytes(),
            )?;
            if plan.run_id != run_id
                || receipt.run_id != run_id
                || receipt.plan_digest != plan_digest
            {
                return Err(SnapshotError::TransactionIntegrity);
            }
            if !matches!(
                receipt.state,
                RestoreState::Verified | RestoreState::RolledBack
            ) {
                plans.push(plan);
            }
        }
        plans.sort_by(|left, right| left.run_id.cmp(&right.run_id));
        Ok(plans)
    }

    fn rollback(
        &self,
        plan: &RestorePlan,
        receipt: &mut RestoreReceipt,
        database_path: &Path,
        lifecycle: &mut impl DatabaseLifecycle,
    ) -> Result<(), SnapshotError> {
        let digest = receipt
            .preimage_object_digest
            .as_deref()
            .ok_or(SnapshotError::TransactionIntegrity)?;
        let encrypted = self.objects.get(digest)?;
        let aad = format!("commonkit.restore-preimage.v1\0{}", plan.run_id);
        let previous = self.cipher.open(&encrypted, aad.as_bytes())?;
        let staged = self
            .root
            .join("runs")
            .join(&plan.run_id)
            .join("rollback.database");
        atomic_write(&staged, &previous)?;
        lifecycle.stop()?;
        replace_file(database_path, &staged)?;
        lifecycle.start()?;
        receipt.state = RestoreState::RolledBack;
        receipt.resulting_content_digest = Some(sha256(&previous));
        self.write_receipt(&self.root.join("runs").join(&plan.run_id), receipt)
    }

    fn write_receipt(&self, run: &Path, receipt: &RestoreReceipt) -> Result<(), SnapshotError> {
        write_authenticated_envelope(
            &run.join("receipt.json"),
            receipt,
            self.cipher,
            restore_auth_aad("receipt", &receipt.run_id).as_bytes(),
        )
        .map(|_| ())
    }
}

fn restore_auth_aad(kind: &str, run_id: &str) -> String {
    format!("commonkit.restore-{kind}-authentication.v1\0{run_id}")
}

fn validate_plan(plan: &RestorePlan, manifest: &SnapshotManifest) -> Result<(), SnapshotError> {
    if plan.schema != "commonkit.restore-plan.v1"
        || plan.database != manifest.database
        || plan.expected_content_digest != manifest.content_digest
        || plan.manifest_digest != manifest_digest(manifest)?
        || plan.run_id.is_empty()
        || plan.snapshot_id.is_empty()
    {
        return Err(SnapshotError::TransactionIntegrity);
    }
    Ok(())
}

pub fn manifest_digest(manifest: &SnapshotManifest) -> Result<String, SnapshotError> {
    serde_jcs::to_vec(manifest)
        .map(|bytes| sha256(&bytes))
        .map_err(|_| SnapshotError::TransactionIntegrity)
}

fn write_envelope<T: Serialize>(path: &Path, value: &T) -> Result<String, SnapshotError> {
    let value_bytes = serde_jcs::to_vec(value).map_err(|_| SnapshotError::TransactionIntegrity)?;
    let digest = sha256(&value_bytes);
    let envelope = IntegrityEnvelope {
        value,
        digest: digest.clone(),
        authentication: None,
    };
    let bytes = serde_jcs::to_vec(&envelope).map_err(|_| SnapshotError::TransactionIntegrity)?;
    atomic_write(path, &bytes)?;
    Ok(digest)
}

fn write_authenticated_envelope<T: Serialize>(
    path: &Path,
    value: &T,
    cipher: &impl AuthenticatedCipher,
    aad: &[u8],
) -> Result<String, SnapshotError> {
    let value_bytes = serde_jcs::to_vec(value).map_err(|_| SnapshotError::TransactionIntegrity)?;
    let digest = sha256(&value_bytes);
    let authentication = cipher.seal(&[], &authentication_aad(aad, &digest))?;
    let envelope = IntegrityEnvelope {
        value,
        digest: digest.clone(),
        authentication: Some(authentication),
    };
    let bytes = serde_jcs::to_vec(&envelope).map_err(|_| SnapshotError::TransactionIntegrity)?;
    atomic_write(path, &bytes)?;
    Ok(digest)
}

fn read_authenticated_envelope<T: Serialize + for<'de> Deserialize<'de>>(
    path: &Path,
    cipher: &impl AuthenticatedCipher,
    aad: &[u8],
) -> Result<(T, String), SnapshotError> {
    let bytes = std::fs::read(path).map_err(|_| SnapshotError::TransactionIntegrity)?;
    let envelope: IntegrityEnvelope<T> =
        serde_json::from_slice(&bytes).map_err(|_| SnapshotError::TransactionIntegrity)?;
    let actual = sha256(
        &serde_jcs::to_vec(&envelope.value).map_err(|_| SnapshotError::TransactionIntegrity)?,
    );
    if actual != envelope.digest {
        return Err(SnapshotError::TransactionIntegrity);
    }
    let authentication = envelope
        .authentication
        .ok_or(SnapshotError::TransactionIntegrity)?;
    if !cipher
        .open(&authentication, &authentication_aad(aad, &envelope.digest))
        .map_err(|_| SnapshotError::TransactionIntegrity)?
        .is_empty()
    {
        return Err(SnapshotError::TransactionIntegrity);
    }
    Ok((envelope.value, envelope.digest))
}

fn authentication_aad(aad: &[u8], digest: &str) -> Vec<u8> {
    let mut output = Vec::with_capacity(aad.len() + 1 + digest.len());
    output.extend_from_slice(aad);
    output.push(0);
    output.extend_from_slice(digest.as_bytes());
    output
}

fn read_envelope<T: Serialize + for<'de> Deserialize<'de>>(
    path: &Path,
) -> Result<(T, String), SnapshotError> {
    let bytes = std::fs::read(path).map_err(|_| SnapshotError::TransactionIntegrity)?;
    let envelope: IntegrityEnvelope<T> =
        serde_json::from_slice(&bytes).map_err(|_| SnapshotError::TransactionIntegrity)?;
    let actual = sha256(
        &serde_jcs::to_vec(&envelope.value).map_err(|_| SnapshotError::TransactionIntegrity)?,
    );
    if actual != envelope.digest {
        return Err(SnapshotError::TransactionIntegrity);
    }
    Ok((envelope.value, envelope.digest))
}

fn validate_digest(value: &str) -> Result<(), SnapshotError> {
    if value.len() != 71
        || !value.starts_with("sha256:")
        || !value[7..]
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(SnapshotError::IntegrityMismatch);
    }
    Ok(())
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), SnapshotError> {
    let parent = path.parent().ok_or(SnapshotError::ObjectStoreFailed)?;
    std::fs::create_dir_all(parent).map_err(|_| SnapshotError::ObjectStoreFailed)?;
    let temporary = path.with_extension("commonkit.tmp");
    std::fs::write(&temporary, bytes).map_err(|_| SnapshotError::ObjectStoreFailed)?;
    std::fs::rename(temporary, path).map_err(|_| SnapshotError::ObjectStoreFailed)
}

fn replace_file(destination: &Path, staged: &Path) -> Result<(), SnapshotError> {
    let replacement = destination.with_extension("commonkit-replacement");
    std::fs::rename(staged, &replacement).map_err(|_| SnapshotError::DatabaseFailed)?;
    if destination.exists() {
        std::fs::remove_file(destination).map_err(|_| SnapshotError::DatabaseFailed)?;
    }
    std::fs::rename(replacement, destination).map_err(|_| SnapshotError::DatabaseFailed)
}

fn verify_database(
    path: &Path,
    expected_digest: &str,
    manifest: &SnapshotManifest,
) -> Result<(), SnapshotError> {
    let bytes = std::fs::read(path).map_err(|_| SnapshotError::DatabaseFailed)?;
    if sha256(&bytes) != expected_digest {
        return Err(SnapshotError::IntegrityMismatch);
    }
    if manifest.source_format.starts_with("sqlite") {
        let connection =
            rusqlite::Connection::open(path).map_err(|_| SnapshotError::DatabaseFailed)?;
        validate_sqlite(&connection)?;
    }
    Ok(())
}

pub trait ConsistentBackup {
    fn source_format(&self) -> &str;
    fn export(&self) -> Result<Vec<u8>, SnapshotError>;
}

/// Uses SQLite's online backup API to obtain a consistent database image. WAL/SHM files are
/// consumed by SQLite and are never included in the exported bytes.
pub struct SqliteBackup {
    source: PathBuf,
}

impl SqliteBackup {
    pub fn new(source: impl AsRef<Path>) -> Self {
        Self {
            source: source.as_ref().to_path_buf(),
        }
    }
}

impl ConsistentBackup for SqliteBackup {
    fn source_format(&self) -> &str {
        "sqlite3-online-backup"
    }

    fn export(&self) -> Result<Vec<u8>, SnapshotError> {
        use rusqlite::backup::Backup;
        use rusqlite::{Connection, OpenFlags};

        let source = Connection::open_with_flags(
            &self.source,
            OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )
        .map_err(|_| SnapshotError::DatabaseFailed)?;
        validate_sqlite(&source)?;
        let directory = tempfile::tempdir().map_err(|_| SnapshotError::DatabaseFailed)?;
        let destination_path = directory.path().join("snapshot.sqlite");
        let mut destination =
            Connection::open(&destination_path).map_err(|_| SnapshotError::DatabaseFailed)?;
        Backup::new(&source, &mut destination)
            .and_then(|backup| backup.run_to_completion(64, Duration::from_millis(5), None))
            .map_err(|_| SnapshotError::DatabaseFailed)?;
        validate_sqlite(&destination)?;
        drop(destination);
        std::fs::read(destination_path).map_err(|_| SnapshotError::DatabaseFailed)
    }
}

fn validate_sqlite(connection: &rusqlite::Connection) -> Result<(), SnapshotError> {
    let result: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|_| SnapshotError::DatabaseFailed)?;
    if result == "ok" {
        Ok(())
    } else {
        Err(SnapshotError::DatabaseFailed)
    }
}

pub trait AuthenticatedCipher {
    fn algorithm(&self) -> &str;
    fn seal(&self, plaintext: &[u8], associated_data: &[u8]) -> Result<Vec<u8>, SnapshotError>;
    fn open(&self, ciphertext: &[u8], associated_data: &[u8]) -> Result<Vec<u8>, SnapshotError>;
}

/// Production snapshot cipher. Ciphertext is `24-byte nonce || authenticated ciphertext`.
/// The key is target-local material and is never serializable or exposed through formatting.
pub struct XChaCha20Cipher([u8; 32]);

impl XChaCha20Cipher {
    pub fn new(key: [u8; 32]) -> Self {
        Self(key)
    }
}

impl Drop for XChaCha20Cipher {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for XChaCha20Cipher {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("XChaCha20Cipher(<redacted>)")
    }
}

impl AuthenticatedCipher for XChaCha20Cipher {
    fn algorithm(&self) -> &str {
        "XCHACHA20-POLY1305"
    }

    fn seal(&self, plaintext: &[u8], associated_data: &[u8]) -> Result<Vec<u8>, SnapshotError> {
        let cipher = XChaCha20Poly1305::new((&self.0).into());
        let mut nonce = [0_u8; 24];
        rand::rng().fill_bytes(&mut nonce);
        let body = cipher
            .encrypt(
                XNonce::from_slice(&nonce),
                Payload {
                    msg: plaintext,
                    aad: associated_data,
                },
            )
            .map_err(|_| SnapshotError::AuthenticationFailed)?;
        let mut output = Vec::with_capacity(nonce.len() + body.len());
        output.extend_from_slice(&nonce);
        output.extend_from_slice(&body);
        nonce.zeroize();
        Ok(output)
    }

    fn open(&self, ciphertext: &[u8], associated_data: &[u8]) -> Result<Vec<u8>, SnapshotError> {
        if ciphertext.len() < 24 + 16 {
            return Err(SnapshotError::AuthenticationFailed);
        }
        let (nonce, body) = ciphertext.split_at(24);
        XChaCha20Poly1305::new((&self.0).into())
            .decrypt(
                XNonce::from_slice(nonce),
                Payload {
                    msg: body,
                    aad: associated_data,
                },
            )
            .map_err(|_| SnapshotError::AuthenticationFailed)
    }
}

pub trait ObjectStore {
    fn put(&mut self, key: &str, ciphertext: &[u8]) -> Result<(), SnapshotError>;
    fn get(&self, key: &str) -> Result<Vec<u8>, SnapshotError>;
}

pub trait ObjectCommandRunner {
    fn run(&mut self, arguments: &[String], stdin: &[u8]) -> Result<Vec<u8>, SnapshotError>;
}

pub struct ProcessObjectCommandRunner {
    executable: PathBuf,
}

impl ProcessObjectCommandRunner {
    pub fn new(executable: impl Into<PathBuf>) -> Self {
        Self {
            executable: executable.into(),
        }
    }
}

impl ObjectCommandRunner for ProcessObjectCommandRunner {
    fn run(&mut self, arguments: &[String], stdin: &[u8]) -> Result<Vec<u8>, SnapshotError> {
        let mut child = Command::new(&self.executable)
            .args(arguments)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|_| SnapshotError::ObjectStoreFailed)?;
        if !stdin.is_empty() {
            use std::io::Write;
            child
                .stdin
                .as_mut()
                .ok_or(SnapshotError::ObjectStoreFailed)?
                .write_all(stdin)
                .map_err(|_| SnapshotError::ObjectStoreFailed)?;
        }
        let output = child
            .wait_with_output()
            .map_err(|_| SnapshotError::ObjectStoreFailed)?;
        if output.status.success() {
            Ok(output.stdout)
        } else {
            Err(SnapshotError::ObjectStoreFailed)
        }
    }
}

/// S3-compatible object storage through the AWS CLI's binary-safe stdin/stdout interface.
/// Credentials remain in the CLI's external credential chain and never enter CommonKit state.
pub struct S3CompatibleObjectStore<R> {
    runner: Mutex<R>,
    endpoint: String,
    bucket: String,
    prefix: String,
}

impl<R> S3CompatibleObjectStore<R> {
    pub fn new(
        runner: R,
        endpoint: impl Into<String>,
        bucket: impl Into<String>,
        prefix: impl Into<String>,
    ) -> Result<Self, SnapshotError> {
        let endpoint = endpoint.into();
        let bucket = bucket.into();
        let prefix = prefix.into().trim_matches('/').to_owned();
        if !endpoint.starts_with("https://")
            || bucket.is_empty()
            || !bucket
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'-'))
            || prefix.split('/').any(|part| part == "." || part == "..")
        {
            return Err(SnapshotError::InvalidObjectStore);
        }
        Ok(Self {
            runner: Mutex::new(runner),
            endpoint,
            bucket,
            prefix,
        })
    }

    fn uri(&self, key: &str) -> Result<String, SnapshotError> {
        if !key.starts_with("sha256:")
            || key.len() != 71
            || !key[7..]
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit() && !byte.is_ascii_uppercase())
        {
            return Err(SnapshotError::InvalidObjectStore);
        }
        let separator = if self.prefix.is_empty() { "" } else { "/" };
        Ok(format!(
            "s3://{}/{}{separator}{key}",
            self.bucket, self.prefix
        ))
    }
}

impl<R: ObjectCommandRunner> ObjectStore for S3CompatibleObjectStore<R> {
    fn put(&mut self, key: &str, ciphertext: &[u8]) -> Result<(), SnapshotError> {
        let uri = self.uri(key)?;
        self.runner
            .get_mut()
            .map_err(|_| SnapshotError::ObjectStoreFailed)?
            .run(
                &[
                    "s3".into(),
                    "cp".into(),
                    "-".into(),
                    uri,
                    "--endpoint-url".into(),
                    self.endpoint.clone(),
                    "--no-progress".into(),
                ],
                ciphertext,
            )?;
        Ok(())
    }

    fn get(&self, key: &str) -> Result<Vec<u8>, SnapshotError> {
        let uri = self.uri(key)?;
        self.runner
            .lock()
            .map_err(|_| SnapshotError::ObjectStoreFailed)?
            .run(
                &[
                    "s3".into(),
                    "cp".into(),
                    uri,
                    "-".into(),
                    "--endpoint-url".into(),
                    self.endpoint.clone(),
                    "--no-progress".into(),
                ],
                &[],
            )
    }
}

/// A target stages imported bytes away from the live database, then swaps them atomically.
pub trait RestoreTarget {
    type Staged;
    type Previous;

    fn stage(&mut self, database: &[u8]) -> Result<Self::Staged, SnapshotError>;
    fn verify_staged(&self, staged: &Self::Staged) -> Result<(), SnapshotError>;
    fn atomic_swap(&mut self, staged: Self::Staged) -> Result<Self::Previous, SnapshotError>;
    fn verify_current(&mut self, expected_digest: &str) -> Result<(), SnapshotError>;
    fn rollback_swap(&mut self, previous: Self::Previous) -> Result<(), SnapshotError>;
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SnapshotManifest {
    pub schema: String,
    pub database: DatabaseId,
    pub source_target: String,
    pub source_format: String,
    pub source_digest: String,
    pub parent_digest: Option<String>,
    pub content_digest: String,
    pub object_digest: String,
    pub cipher: String,
}

pub struct SnapshotService<'a, C> {
    cipher: &'a C,
}

impl<'a, C: AuthenticatedCipher> SnapshotService<'a, C> {
    pub fn new(cipher: &'a C) -> Self {
        Self { cipher }
    }

    pub fn snapshot(
        &self,
        database: &DatabaseId,
        source_target: &str,
        parent_digest: Option<String>,
        backup: &impl ConsistentBackup,
        objects: &mut impl ObjectStore,
    ) -> Result<SnapshotManifest, SnapshotError> {
        let plaintext = backup.export()?;
        let content_digest = sha256(&plaintext);
        let source_format = backup.source_format().to_owned();
        let source_digest = content_digest.clone();
        let cipher = self.cipher.algorithm().to_owned();
        let associated_data = snapshot_aad(
            database,
            source_target,
            &source_format,
            &source_digest,
            parent_digest.as_deref(),
            &content_digest,
            &cipher,
        );
        let ciphertext = self.cipher.seal(&plaintext, associated_data.as_bytes())?;
        let object_digest = sha256(&ciphertext);
        objects.put(&object_digest, &ciphertext)?;
        Ok(SnapshotManifest {
            schema: "commonkit.snapshot.v1".into(),
            database: database.clone(),
            source_target: source_target.into(),
            source_format,
            source_digest,
            parent_digest,
            content_digest,
            object_digest,
            cipher,
        })
    }

    pub fn verify_and_decrypt(
        &self,
        manifest: &SnapshotManifest,
        objects: &impl ObjectStore,
    ) -> Result<Vec<u8>, SnapshotError> {
        if manifest.schema != "commonkit.snapshot.v1" || manifest.cipher != self.cipher.algorithm()
        {
            return Err(SnapshotError::UnsupportedSnapshotFormat);
        }
        let ciphertext = objects.get(&manifest.object_digest)?;
        if sha256(&ciphertext) != manifest.object_digest {
            return Err(SnapshotError::IntegrityMismatch);
        }
        let associated_data = snapshot_aad(
            &manifest.database,
            &manifest.source_target,
            &manifest.source_format,
            &manifest.source_digest,
            manifest.parent_digest.as_deref(),
            &manifest.content_digest,
            &manifest.cipher,
        );
        let plaintext = self.cipher.open(&ciphertext, associated_data.as_bytes())?;
        if sha256(&plaintext) != manifest.content_digest {
            return Err(SnapshotError::IntegrityMismatch);
        }
        Ok(plaintext)
    }

    pub fn restore(
        &self,
        manifest: &SnapshotManifest,
        objects: &impl ObjectStore,
        target: &mut impl RestoreTarget,
    ) -> Result<(), SnapshotError> {
        let plaintext = self.verify_and_decrypt(manifest, objects)?;
        let staged = target.stage(&plaintext)?;
        target.verify_staged(&staged)?;
        let previous = target.atomic_swap(staged)?;
        if target.verify_current(&manifest.content_digest).is_err() {
            target.rollback_swap(previous)?;
            return Err(SnapshotError::RestoreVerificationFailed);
        }
        Ok(())
    }
}

fn sha256(bytes: &[u8]) -> String {
    format!("sha256:{:x}", Sha256::digest(bytes))
}

fn copy_portable_tree(source: &Path, destination: &Path) -> Result<(), SnapshotError> {
    std::fs::create_dir(destination).map_err(|_| SnapshotError::ObjectStoreFailed)?;
    for entry in std::fs::read_dir(source).map_err(|_| SnapshotError::ObjectStoreFailed)? {
        let entry = entry.map_err(|_| SnapshotError::ObjectStoreFailed)?;
        let metadata = std::fs::symlink_metadata(entry.path())
            .map_err(|_| SnapshotError::ObjectStoreFailed)?;
        if metadata.file_type().is_symlink() {
            return Err(SnapshotError::TransactionIntegrity);
        }
        let target = destination.join(entry.file_name());
        if metadata.is_dir() {
            copy_portable_tree(&entry.path(), &target)?;
        } else if metadata.is_file() {
            copy_verified_file(&entry.path(), &target)?;
        } else {
            return Err(SnapshotError::TransactionIntegrity);
        }
    }
    Ok(())
}

fn copy_verified_file(source: &Path, destination: &Path) -> Result<(), SnapshotError> {
    let bytes = std::fs::read(source).map_err(|_| SnapshotError::ObjectStoreFailed)?;
    if let Some(parent) = destination.parent() {
        std::fs::create_dir_all(parent).map_err(|_| SnapshotError::ObjectStoreFailed)?;
    }
    let temporary = destination.with_extension(format!("tmp-{:032x}", rand::random::<u128>()));
    std::fs::write(&temporary, &bytes).map_err(|_| SnapshotError::ObjectStoreFailed)?;
    std::fs::rename(&temporary, destination).map_err(|_| SnapshotError::ObjectStoreFailed)?;
    Ok(())
}

fn safe_relative_path(path: &Path) -> bool {
    !path.as_os_str().is_empty()
        && !path.is_absolute()
        && path.components().all(|component| {
            matches!(
                component,
                std::path::Component::Normal(value) if !value.is_empty()
            )
        })
}

fn safe_git_branch(branch: &str) -> bool {
    !branch.is_empty()
        && !branch.starts_with('-')
        && !branch.starts_with('/')
        && !branch.ends_with('/')
        && !branch.ends_with('.')
        && !branch.contains("..")
        && !branch.contains("@{")
        && branch
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b'/'))
}

fn valid_git_revision(revision: &str) -> bool {
    matches!(revision.len(), 40 | 64)
        && revision
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn snapshot_aad(
    database: &DatabaseId,
    source_target: &str,
    source_format: &str,
    source_digest: &str,
    parent_digest: Option<&str>,
    content_digest: &str,
    cipher: &str,
) -> String {
    format!(
        "commonkit.snapshot.v1\0{database}\0{source_target}\0{source_format}\0{source_digest}\0{}\0{content_digest}\0{cipher}",
        parent_digest.unwrap_or("")
    )
}

#[derive(Clone, Debug)]
pub struct StaticBackup {
    format: String,
    bytes: Vec<u8>,
}

impl StaticBackup {
    pub fn new(format: impl Into<String>, bytes: Vec<u8>) -> Self {
        Self {
            format: format.into(),
            bytes,
        }
    }
}

impl ConsistentBackup for StaticBackup {
    fn source_format(&self) -> &str {
        &self.format
    }
    fn export(&self) -> Result<Vec<u8>, SnapshotError> {
        Ok(self.bytes.clone())
    }
}

#[derive(Default)]
pub struct InMemoryObjectStore(BTreeMap<String, Vec<u8>>);

impl InMemoryObjectStore {
    pub fn contains_plaintext(&self, needle: &[u8]) -> bool {
        self.0
            .values()
            .any(|value| value.windows(needle.len()).any(|window| window == needle))
    }
}

impl ObjectStore for InMemoryObjectStore {
    fn put(&mut self, key: &str, ciphertext: &[u8]) -> Result<(), SnapshotError> {
        self.0
            .entry(key.into())
            .or_insert_with(|| ciphertext.to_vec());
        Ok(())
    }
    fn get(&self, key: &str) -> Result<Vec<u8>, SnapshotError> {
        self.0
            .get(key)
            .cloned()
            .ok_or_else(|| SnapshotError::ObjectNotFound(key.into()))
    }
}

/// Deterministic authenticated test cipher. It is intentionally not suitable for production.
pub struct DeterministicTestCipher([u8; 32]);

impl DeterministicTestCipher {
    pub fn new(key: [u8; 32]) -> Self {
        Self(key)
    }
}

impl AuthenticatedCipher for DeterministicTestCipher {
    fn algorithm(&self) -> &str {
        "INSECURE-TEST-XOR-SHA256"
    }
    fn seal(&self, plaintext: &[u8], associated_data: &[u8]) -> Result<Vec<u8>, SnapshotError> {
        let body: Vec<_> = plaintext
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ self.0[index % 32])
            .collect();
        let mut hash = Sha256::new();
        hash.update(self.0);
        hash.update(associated_data);
        hash.update(&body);
        let mut result = hash.finalize().to_vec();
        result.extend(body);
        Ok(result)
    }
    fn open(&self, ciphertext: &[u8], associated_data: &[u8]) -> Result<Vec<u8>, SnapshotError> {
        if ciphertext.len() < 32 {
            return Err(SnapshotError::AuthenticationFailed);
        }
        let (tag, body) = ciphertext.split_at(32);
        let mut hash = Sha256::new();
        hash.update(self.0);
        hash.update(associated_data);
        hash.update(body);
        if hash.finalize().as_slice() != tag {
            return Err(SnapshotError::AuthenticationFailed);
        }
        Ok(body
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ self.0[index % 32])
            .collect())
    }
}

/// In-memory restore target used to contract-test staging and rollback behavior.
pub struct InMemoryDatabaseTarget {
    current: Vec<u8>,
    swaps: usize,
    fail_verification: bool,
}

impl InMemoryDatabaseTarget {
    pub fn new(current: Vec<u8>) -> Self {
        Self {
            current,
            swaps: 0,
            fail_verification: false,
        }
    }
    pub fn current(&self) -> &[u8] {
        &self.current
    }
    pub fn swap_count(&self) -> usize {
        self.swaps
    }
    pub fn fail_next_verification(&mut self) {
        self.fail_verification = true;
    }
}

impl RestoreTarget for InMemoryDatabaseTarget {
    type Staged = Vec<u8>;
    type Previous = Vec<u8>;

    fn stage(&mut self, database: &[u8]) -> Result<Self::Staged, SnapshotError> {
        Ok(database.to_vec())
    }
    fn verify_staged(&self, _staged: &Self::Staged) -> Result<(), SnapshotError> {
        Ok(())
    }
    fn atomic_swap(&mut self, staged: Self::Staged) -> Result<Self::Previous, SnapshotError> {
        self.swaps += 1;
        Ok(std::mem::replace(&mut self.current, staged))
    }
    fn verify_current(&mut self, expected_digest: &str) -> Result<(), SnapshotError> {
        if std::mem::take(&mut self.fail_verification) || sha256(&self.current) != expected_digest {
            Err(SnapshotError::IntegrityMismatch)
        } else {
            Ok(())
        }
    }
    fn rollback_swap(&mut self, previous: Self::Previous) -> Result<(), SnapshotError> {
        self.current = previous;
        Ok(())
    }
}
