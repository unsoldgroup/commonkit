use std::collections::{BTreeMap, BTreeSet};

use commonkit_contracts::{SchemaVersion, Sha256Digest, StableId, assert_no_embedded_secrets};
use serde::{Deserialize, Serialize};
use thiserror::Error;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ValidationPlatform {
    MacOs,
    Linux,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvidenceOutcome {
    Passed,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeValidationStep {
    pub id: StableId,
    pub outcome: EvidenceOutcome,
    pub artifact_hash: Sha256Digest,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct NativeValidationEvidence {
    pub schema_version: SchemaVersion,
    pub platform: ValidationPlatform,
    pub architecture: String,
    pub repository_revision: Sha256Digest,
    pub environment_digest: Sha256Digest,
    pub provider_digests: BTreeMap<StableId, Sha256Digest>,
    pub steps: Vec<NativeValidationStep>,
    pub recorded_at_unix_ms: u64,
}

impl NativeValidationEvidence {
    pub fn validate(&self, expected_revision: &Sha256Digest) -> Result<(), NativeValidationError> {
        if &self.repository_revision != expected_revision {
            return Err(NativeValidationError::RevisionMismatch);
        }
        if self.architecture.trim().is_empty()
            || self.recorded_at_unix_ms == 0
            || self.steps.is_empty()
            || self
                .steps
                .iter()
                .any(|step| step.outcome != EvidenceOutcome::Passed)
        {
            return Err(NativeValidationError::IncompleteEvidence);
        }
        let value = serde_json::to_value(self).map_err(|_| NativeValidationError::InvalidRecord)?;
        assert_no_embedded_secrets(&value).map_err(|_| NativeValidationError::SecretBearing)?;
        Ok(())
    }
}

pub fn portable_context_support_ready(
    records: &[NativeValidationEvidence],
    expected_revision: &Sha256Digest,
) -> bool {
    let mut passing = BTreeSet::new();
    for record in records {
        if record.validate(expected_revision).is_ok() && !passing.insert(record.platform) {
            return false;
        }
    }
    passing
        == [
            ValidationPlatform::MacOs,
            ValidationPlatform::Linux,
            ValidationPlatform::Windows,
        ]
        .into_iter()
        .collect()
}

#[derive(Debug, Error, Clone, Copy, PartialEq, Eq)]
pub enum NativeValidationError {
    #[error("native validation evidence belongs to another revision")]
    RevisionMismatch,
    #[error("native validation evidence is incomplete or not passing")]
    IncompleteEvidence,
    #[error("native validation evidence is invalid")]
    InvalidRecord,
    #[error("native validation evidence contains secret-bearing metadata")]
    SecretBearing,
}
