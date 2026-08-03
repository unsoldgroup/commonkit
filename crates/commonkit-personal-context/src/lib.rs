//! Provider-neutral encryption for user-owned portable profile revisions.

use std::{
    collections::BTreeMap,
    fmt,
    str::FromStr,
    sync::atomic::{AtomicU64, Ordering},
};

use age::x25519;
use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64};
use chacha20poly1305::{
    XChaCha20Poly1305, XNonce,
    aead::{Aead, KeyInit, Payload},
};
use commonkit_contracts::{
    SchemaVersion, Sha256Digest, StableId, canonical_json, digest_domain_json,
};
use rand::RngCore;
use serde::{Deserialize, Serialize};
use thiserror::Error;
use zeroize::Zeroize;

pub struct EncryptedRevisionStore {
    root: std::path::PathBuf,
}

static STAGED_REVISION_TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

impl EncryptedRevisionStore {
    pub fn open(root: impl AsRef<std::path::Path>) -> Result<Self, CryptoError> {
        let root = root.as_ref().to_path_buf();
        if !root.is_absolute() {
            return Err(CryptoError::UnsafeStore);
        }
        commonkit_platform::ensure_private_path(
            &root,
            commonkit_platform::PrivatePathKind::Directory,
        )
        .map_err(|_| CryptoError::UnsafeStore)?;
        Ok(Self { root })
    }

    pub fn stage(&self, revision: &EncryptedRevision) -> Result<Sha256Digest, CryptoError> {
        let digest = digest_domain_json("commonkit.personal-context.staged-revision.v1", revision)
            .map_err(|_| CryptoError::Canonicalization)?;
        let path = self
            .root
            .join(format!("{}.json", revision.binding.revision_id.as_str()));
        let bytes = serde_json::to_vec(revision).map_err(|_| CryptoError::Canonicalization)?;
        if path.exists() {
            commonkit_platform::verify_private_path(
                &path,
                commonkit_platform::PrivatePathKind::File,
            )
            .map_err(|_| CryptoError::UnsafeStore)?;
            return if std::fs::read(&path).map_err(|_| CryptoError::StoreIo)? == bytes {
                Ok(digest)
            } else {
                Err(CryptoError::RevisionCollision)
            };
        }
        let temporary = self.root.join(format!(
            ".{}.{}.{}.tmp",
            revision.binding.revision_id.as_str(),
            std::process::id(),
            STAGED_REVISION_TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        commonkit_platform::ensure_private_path(
            &temporary,
            commonkit_platform::PrivatePathKind::File,
        )
        .map_err(|_| CryptoError::UnsafeStore)?;
        let result = (|| {
            use std::io::Write as _;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .truncate(true)
                .open(&temporary)
                .map_err(|_| CryptoError::StoreIo)?;
            file.write_all(&bytes).map_err(|_| CryptoError::StoreIo)?;
            file.sync_all().map_err(|_| CryptoError::StoreIo)?;
            if std::fs::rename(&temporary, &path).is_err() {
                commonkit_platform::verify_private_path(
                    &path,
                    commonkit_platform::PrivatePathKind::File,
                )
                .map_err(|_| CryptoError::UnsafeStore)?;
                if std::fs::read(&path).map_err(|_| CryptoError::StoreIo)? != bytes {
                    return Err(CryptoError::RevisionCollision);
                }
                std::fs::remove_file(&temporary).map_err(|_| CryptoError::StoreIo)?;
            }
            std::fs::File::open(&self.root)
                .and_then(|directory| directory.sync_all())
                .map_err(|_| CryptoError::StoreIo)?;
            Ok::<_, CryptoError>(())
        })();
        if result.is_err() {
            let _ = std::fs::remove_file(temporary);
        }
        result.map(|()| digest)
    }
}

const MAX_SECRET_BYTES: usize = 64 * 1024;
const MAX_FIELDS: usize = 256;

pub struct SecretValue(Vec<u8>);

impl SecretValue {
    pub fn new(value: impl AsRef<[u8]>) -> Self {
        Self(value.as_ref().to_vec())
    }

    pub fn from_string(value: String) -> Self {
        Self(value.into_bytes())
    }

    fn bytes(&self) -> &[u8] {
        &self.0
    }

    /// Exposes plaintext only to an explicitly authorized local operation.
    pub fn expose(&self) -> &[u8] {
        self.bytes()
    }
}

impl Drop for SecretValue {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

impl fmt::Debug for SecretValue {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("SecretValue([REDACTED])")
    }
}

impl PartialEq for SecretValue {
    fn eq(&self, other: &Self) -> bool {
        self.0 == other.0
    }
}

impl Eq for SecretValue {}

#[derive(Debug, PartialEq, Eq)]
pub enum FieldOperation {
    Set(SecretValue),
    Delete,
}

#[derive(Debug, PartialEq, Eq)]
pub struct DecryptedValue(Option<SecretValue>);

impl DecryptedValue {
    pub fn expose(&self) -> Option<&[u8]> {
        self.0.as_ref().map(SecretValue::bytes)
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ConfirmedProfileValue {
    pub field_id: String,
    pub value: SecretValue,
}

#[derive(Debug, PartialEq, Eq)]
pub struct ConfirmedProfileDraft {
    pub values: Vec<ConfirmedProfileValue>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterviewCancellation {
    pub portable_writes: usize,
    pub persisted_transcript_bytes: usize,
}

pub struct ProfileInterview {
    field_order: Vec<String>,
    index: usize,
    pending: Option<SecretValue>,
    pending_digest: Option<Sha256Digest>,
    confirmed: Vec<ConfirmedProfileValue>,
}

impl ProfileInterview {
    pub fn new(schema: commonkit_contracts::portable_context::ProfileSchema) -> Self {
        let mut field_order = commonkit_contracts::portable_context::CORE_PROFILE_FIELD_ORDER
            .iter()
            .filter(|field| schema.fields.contains_key(**field))
            .map(|field| (*field).to_owned())
            .collect::<Vec<_>>();
        let extension_fields = schema
            .fields
            .keys()
            .filter(|field| !field_order.contains(field))
            .cloned()
            .collect::<Vec<_>>();
        field_order.extend(extension_fields);
        Self {
            field_order,
            index: 0,
            pending: None,
            pending_digest: None,
            confirmed: Vec::new(),
        }
    }

    pub fn next_field(&self) -> Option<&str> {
        self.field_order.get(self.index).map(String::as_str)
    }

    pub fn answer_current(&mut self, value: SecretValue) -> Result<(), InterviewError> {
        let field = self.next_field().ok_or(InterviewError::Complete)?;
        let digest = digest_domain_json(
            "commonkit.profile-interview-confirmation.v1",
            &(field, value.bytes()),
        )
        .map_err(|_| InterviewError::Canonicalization)?;
        self.pending = Some(value);
        self.pending_digest = Some(digest);
        Ok(())
    }

    pub fn pending_confirmation(&self) -> Option<Sha256Digest> {
        self.pending_digest.clone()
    }

    pub fn confirm_pending(&mut self, digest: &Sha256Digest) -> Result<(), InterviewError> {
        if self.pending_digest.as_ref() != Some(digest) {
            return Err(InterviewError::ConfirmationMismatch);
        }
        let value = self.pending.take().ok_or(InterviewError::NoPendingValue)?;
        let field_id = self
            .next_field()
            .ok_or(InterviewError::Complete)?
            .to_owned();
        self.confirmed
            .push(ConfirmedProfileValue { field_id, value });
        self.pending_digest = None;
        Ok(())
    }

    pub fn advance(&mut self) -> Result<(), InterviewError> {
        if self.pending.is_some() {
            return Err(InterviewError::UnconfirmedValue);
        }
        self.index = self.index.saturating_add(1).min(self.field_order.len());
        Ok(())
    }

    pub fn into_confirmed_draft(self) -> Result<ConfirmedProfileDraft, InterviewError> {
        if self.pending.is_some() {
            return Err(InterviewError::UnconfirmedValue);
        }
        Ok(ConfirmedProfileDraft {
            values: self.confirmed,
        })
    }

    pub fn cancel(self) -> InterviewCancellation {
        drop(self);
        InterviewCancellation {
            portable_writes: 0,
            persisted_transcript_bytes: 0,
        }
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum InterviewError {
    #[error("profile interview is complete")]
    Complete,
    #[error("profile value has not been confirmed")]
    UnconfirmedValue,
    #[error("profile interview has no pending value")]
    NoPendingValue,
    #[error("profile confirmation does not match the pending value")]
    ConfirmationMismatch,
    #[error("profile confirmation could not be canonicalized")]
    Canonicalization,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DeletionDisposition {
    Applied,
    AlreadyApplied,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct OpaqueDeletionReceipt {
    pub profile_id: StableId,
    pub tombstone_generation: u64,
    pub disposition: DeletionDisposition,
    pub removed_ciphertext_objects: usize,
    pub removed_cache_entries: usize,
    pub revoked_grants: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PersonalContextDeletionState {
    pub schema_version: SchemaVersion,
    pub profile_id: StableId,
    pub latest_generation: u64,
    pub tombstone_generation: Option<u64>,
    pub active_ciphertext_objects: usize,
    pub controlled_cache_entries: usize,
    pub active_grants: usize,
}

impl PersonalContextDeletionState {
    pub fn observe_tombstone(
        &mut self,
        generation: u64,
    ) -> Result<OpaqueDeletionReceipt, DeletionError> {
        if self.tombstone_generation == Some(generation) {
            return Ok(OpaqueDeletionReceipt {
                profile_id: self.profile_id.clone(),
                tombstone_generation: generation,
                disposition: DeletionDisposition::AlreadyApplied,
                removed_ciphertext_objects: 0,
                removed_cache_entries: 0,
                revoked_grants: 0,
            });
        }
        if generation <= self.latest_generation
            || self
                .tombstone_generation
                .is_some_and(|observed| generation <= observed)
        {
            return Err(DeletionError::NonmonotonicTombstone);
        }
        let receipt = OpaqueDeletionReceipt {
            profile_id: self.profile_id.clone(),
            tombstone_generation: generation,
            disposition: DeletionDisposition::Applied,
            removed_ciphertext_objects: self.active_ciphertext_objects,
            removed_cache_entries: self.controlled_cache_entries,
            revoked_grants: self.active_grants,
        };
        self.active_ciphertext_objects = 0;
        self.controlled_cache_entries = 0;
        self.active_grants = 0;
        self.tombstone_generation = Some(generation);
        self.latest_generation = generation;
        Ok(receipt)
    }

    pub fn accepts_revision(&self, profile_id: &str, generation: u64) -> bool {
        if profile_id != self.profile_id.as_str() {
            return generation > 0;
        }
        self.tombstone_generation.is_none() && generation > self.latest_generation
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct PersonalContextLease {
    pub issued_at_unix_ms: u64,
    pub expires_at_unix_ms: u64,
    pub revocation_generation: u64,
}

impl PersonalContextLease {
    pub fn authorizes(self, now_unix_ms: u64, current_revocation_generation: u64) -> bool {
        self.issued_at_unix_ms <= now_unix_ms
            && now_unix_ms < self.expires_at_unix_ms
            && self.revocation_generation == current_revocation_generation
    }
}

#[derive(Debug, Error, PartialEq, Eq)]
pub enum DeletionError {
    #[error("deletion tombstone generation must advance the profile history")]
    NonmonotonicTombstone,
}

pub const SIX_MONTH_REVIEW_MS: u64 = 183 * 24 * 60 * 60 * 1_000;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ProfileReviewSchedule {
    pub last_reviewed_at_unix_ms: u64,
    pub global_dismissed_until_unix_ms: Option<u64>,
    pub section_dismissed_until_unix_ms: BTreeMap<String, u64>,
}

impl ProfileReviewSchedule {
    pub fn is_due(&self, section: &str, now_unix_ms: u64, material_change: bool) -> bool {
        if self
            .global_dismissed_until_unix_ms
            .is_some_and(|until| now_unix_ms < until)
            || self
                .section_dismissed_until_unix_ms
                .get(section)
                .is_some_and(|until| now_unix_ms < *until)
        {
            return false;
        }
        material_change
            || now_unix_ms
                >= self
                    .last_reviewed_at_unix_ms
                    .saturating_add(SIX_MONTH_REVIEW_MS)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RevisionBinding {
    pub schema_version: SchemaVersion,
    pub profile_id: StableId,
    pub profile_schema_id: StableId,
    pub profile_schema_version: u32,
    pub revision_id: StableId,
    pub parent_hashes: Vec<Sha256Digest>,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize)]
#[serde(transparent)]
pub struct ProfileFieldId(String);

impl ProfileFieldId {
    pub fn parse(value: impl Into<String>) -> Result<Self, CryptoError> {
        let value = value.into();
        let valid_length = !value.is_empty() && value.len() <= 127;
        let mut segments = value.split('.');
        let valid_segments = segments.all(|segment| {
            let mut characters = segment.chars();
            characters
                .next()
                .is_some_and(|character| character.is_ascii_lowercase())
                && characters.all(|character| {
                    character.is_ascii_lowercase()
                        || character.is_ascii_digit()
                        || matches!(character, '_' | '-')
                })
        });
        if valid_length && valid_segments {
            Ok(Self(value))
        } else {
            Err(CryptoError::InvalidFieldId)
        }
    }

    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl<'de> Deserialize<'de> for ProfileFieldId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        Self::parse(String::deserialize(deserializer)?).map_err(serde::de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EncryptedOperation {
    Set,
    Delete,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct WrappedDataKey {
    pub recipient: String,
    pub ciphertext: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EncryptedField {
    pub field_id: ProfileFieldId,
    pub operation: EncryptedOperation,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nonce: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ciphertext: Option<String>,
    pub binding_digest: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct EncryptedRevision {
    pub schema_version: SchemaVersion,
    pub binding: RevisionBinding,
    pub recipient_set_digest: Sha256Digest,
    pub wrapped_data_keys: Vec<WrappedDataKey>,
    pub fields: Vec<EncryptedField>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FieldConflict {
    pub field_id: ProfileFieldId,
    pub left: EncryptedField,
    pub right: EncryptedField,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeOutcome {
    pub merged: BTreeMap<ProfileFieldId, EncryptedField>,
    pub conflicts: Vec<FieldConflict>,
}

pub fn merge_concurrent_operations(
    left: BTreeMap<ProfileFieldId, EncryptedField>,
    mut right: BTreeMap<ProfileFieldId, EncryptedField>,
) -> MergeOutcome {
    let mut merged = BTreeMap::new();
    let mut conflicts = Vec::new();
    for (field_id, left_field) in left {
        match right.remove(&field_id) {
            Some(right_field) if right_field != left_field => conflicts.push(FieldConflict {
                field_id,
                left: left_field,
                right: right_field,
            }),
            Some(field) => {
                merged.insert(field_id, field);
            }
            None => {
                merged.insert(field_id, left_field);
            }
        }
    }
    merged.extend(right);
    MergeOutcome { merged, conflicts }
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct LeafBinding<'a> {
    revision: &'a RevisionBinding,
    field_id: &'a ProfileFieldId,
    operation: EncryptedOperation,
    recipient_set_digest: &'a Sha256Digest,
}

#[derive(Debug, Error)]
pub enum CryptoError {
    #[error("at least one recovery recipient is required")]
    NoRecipients,
    #[error("profile revision exceeds the field limit")]
    TooManyFields,
    #[error("profile value exceeds the size limit")]
    ValueTooLarge,
    #[error("profile revision parents must be unique and sorted")]
    NoncanonicalParents,
    #[error("invalid encrypted profile encoding")]
    InvalidEncoding,
    #[error("no supplied identity can recover this profile revision")]
    RecoveryFailed,
    #[error("encrypted profile authentication failed")]
    AuthenticationFailed,
    #[error("portable profile contract could not be canonicalized")]
    Canonicalization,
    #[error("age key wrapping failed")]
    KeyWrapping,
    #[error("encrypted revision store is unsafe")]
    UnsafeStore,
    #[error("encrypted revision store could not be updated")]
    StoreIo,
    #[error("a different revision already uses this revision ID")]
    RevisionCollision,
    #[error("recovery recipient is invalid")]
    InvalidRecipient,
    #[error("profile field ID is invalid")]
    InvalidFieldId,
}

pub fn encrypt_revision_for_recipient_strings(
    binding: RevisionBinding,
    fields: BTreeMap<ProfileFieldId, FieldOperation>,
    recipients: &[String],
) -> Result<EncryptedRevision, CryptoError> {
    let recipients = recipients
        .iter()
        .map(|recipient| {
            x25519::Recipient::from_str(recipient).map_err(|_| CryptoError::InvalidRecipient)
        })
        .collect::<Result<Vec<_>, _>>()?;
    encrypt_revision(binding, fields, &recipients)
}

pub fn encrypt_revision(
    binding: RevisionBinding,
    fields: BTreeMap<ProfileFieldId, FieldOperation>,
    recipients: &[x25519::Recipient],
) -> Result<EncryptedRevision, CryptoError> {
    if recipients.is_empty() {
        return Err(CryptoError::NoRecipients);
    }
    if fields.len() > MAX_FIELDS {
        return Err(CryptoError::TooManyFields);
    }
    if binding
        .parent_hashes
        .windows(2)
        .any(|pair| pair[0] >= pair[1])
    {
        return Err(CryptoError::NoncanonicalParents);
    }

    let mut recipient_names = recipients
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    recipient_names.sort();
    recipient_names.dedup();
    let recipient_set_digest = digest_domain_json(
        "commonkit.personal-context.recipient-set.v1",
        &recipient_names,
    )
    .map_err(|_| CryptoError::Canonicalization)?;

    let mut data_key = [0_u8; 32];
    rand::rng().fill_bytes(&mut data_key);
    let cipher = XChaCha20Poly1305::new((&data_key).into());

    let result = (|| {
        let wrapped_data_keys = recipients
            .iter()
            .map(|recipient| {
                age::encrypt(recipient, &data_key)
                    .map(|ciphertext| WrappedDataKey {
                        recipient: recipient.to_string(),
                        ciphertext: BASE64.encode(ciphertext),
                    })
                    .map_err(|_| CryptoError::KeyWrapping)
            })
            .collect::<Result<Vec<_>, _>>()?;

        let mut encrypted_fields = Vec::with_capacity(fields.len());
        for (field_id, operation) in fields {
            let operation_kind = match operation {
                FieldOperation::Set(_) => EncryptedOperation::Set,
                FieldOperation::Delete => EncryptedOperation::Delete,
            };
            let leaf_binding = LeafBinding {
                revision: &binding,
                field_id: &field_id,
                operation: operation_kind,
                recipient_set_digest: &recipient_set_digest,
            };
            let aad = canonical_json(&leaf_binding).map_err(|_| CryptoError::Canonicalization)?;
            let binding_digest =
                digest_domain_json("commonkit.personal-context.leaf-binding.v1", &leaf_binding)
                    .map_err(|_| CryptoError::Canonicalization)?;
            let (nonce, ciphertext) = match operation {
                FieldOperation::Set(secret) => {
                    if secret.bytes().len() > MAX_SECRET_BYTES {
                        return Err(CryptoError::ValueTooLarge);
                    }
                    let mut nonce = [0_u8; 24];
                    rand::rng().fill_bytes(&mut nonce);
                    let ciphertext = cipher
                        .encrypt(
                            XNonce::from_slice(&nonce),
                            Payload {
                                msg: secret.bytes(),
                                aad: &aad,
                            },
                        )
                        .map_err(|_| CryptoError::AuthenticationFailed)?;
                    (Some(BASE64.encode(nonce)), Some(BASE64.encode(ciphertext)))
                }
                FieldOperation::Delete => (None, None),
            };
            encrypted_fields.push(EncryptedField {
                field_id,
                operation: operation_kind,
                nonce,
                ciphertext,
                binding_digest,
            });
        }
        Ok(EncryptedRevision {
            schema_version: SchemaVersion(1),
            binding,
            recipient_set_digest,
            wrapped_data_keys,
            fields: encrypted_fields,
        })
    })();
    data_key.zeroize();
    result
}

pub fn decrypt_revision(
    revision: &EncryptedRevision,
    identity: &x25519::Identity,
) -> Result<BTreeMap<ProfileFieldId, DecryptedValue>, CryptoError> {
    let mut recipient_names = revision
        .wrapped_data_keys
        .iter()
        .map(|wrapped| wrapped.recipient.clone())
        .collect::<Vec<_>>();
    recipient_names.sort();
    recipient_names.dedup();
    let recipient_set_digest = digest_domain_json(
        "commonkit.personal-context.recipient-set.v1",
        &recipient_names,
    )
    .map_err(|_| CryptoError::Canonicalization)?;
    if recipient_set_digest != revision.recipient_set_digest {
        return Err(CryptoError::AuthenticationFailed);
    }
    let mut data_key = revision
        .wrapped_data_keys
        .iter()
        .find_map(|wrapped| {
            BASE64
                .decode(&wrapped.ciphertext)
                .ok()
                .and_then(|ciphertext| age::decrypt(identity, &ciphertext).ok())
                .filter(|key| key.len() == 32)
        })
        .ok_or(CryptoError::RecoveryFailed)?;
    let cipher =
        XChaCha20Poly1305::new_from_slice(&data_key).map_err(|_| CryptoError::InvalidEncoding)?;

    let result = (|| {
        let mut fields = BTreeMap::new();
        for field in &revision.fields {
            let leaf_binding = LeafBinding {
                revision: &revision.binding,
                field_id: &field.field_id,
                operation: field.operation,
                recipient_set_digest: &revision.recipient_set_digest,
            };
            let expected =
                digest_domain_json("commonkit.personal-context.leaf-binding.v1", &leaf_binding)
                    .map_err(|_| CryptoError::Canonicalization)?;
            if expected != field.binding_digest {
                return Err(CryptoError::AuthenticationFailed);
            }
            let value = match field.operation {
                EncryptedOperation::Delete => {
                    if field.nonce.is_some() || field.ciphertext.is_some() {
                        return Err(CryptoError::InvalidEncoding);
                    }
                    DecryptedValue(None)
                }
                EncryptedOperation::Set => {
                    let nonce = BASE64
                        .decode(field.nonce.as_ref().ok_or(CryptoError::InvalidEncoding)?)
                        .map_err(|_| CryptoError::InvalidEncoding)?;
                    let ciphertext = BASE64
                        .decode(
                            field
                                .ciphertext
                                .as_ref()
                                .ok_or(CryptoError::InvalidEncoding)?,
                        )
                        .map_err(|_| CryptoError::InvalidEncoding)?;
                    if nonce.len() != 24 || ciphertext.len() > MAX_SECRET_BYTES + 16 {
                        return Err(CryptoError::InvalidEncoding);
                    }
                    let aad =
                        canonical_json(&leaf_binding).map_err(|_| CryptoError::Canonicalization)?;
                    let plaintext = cipher
                        .decrypt(
                            XNonce::from_slice(&nonce),
                            Payload {
                                msg: &ciphertext,
                                aad: &aad,
                            },
                        )
                        .map_err(|_| CryptoError::AuthenticationFailed)?;
                    DecryptedValue(Some(SecretValue(plaintext)))
                }
            };
            fields.insert(field.field_id.clone(), value);
        }
        Ok(fields)
    })();
    data_key.zeroize();
    result
}

pub fn rotate_recipients(
    revision: &EncryptedRevision,
    current_identity: &x25519::Identity,
    new_recipients: &[x25519::Recipient],
) -> Result<EncryptedRevision, CryptoError> {
    let plaintext = decrypt_revision(revision, current_identity)?;
    let operations = plaintext
        .into_iter()
        .map(|(field_id, value)| {
            let operation = match value.0 {
                Some(secret) => FieldOperation::Set(secret),
                None => FieldOperation::Delete,
            };
            (field_id, operation)
        })
        .collect();
    encrypt_revision(revision.binding.clone(), operations, new_recipients)
}
