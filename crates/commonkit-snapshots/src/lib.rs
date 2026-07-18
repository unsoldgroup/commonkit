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
